# Troubleshooting

Refusals a newcomer actually hits, in the [Tutorial](tutorial.md)'s own sequence, each quoted
verbatim from the CLI. If a message here reads as confusing, that is worth reporting — this page
explains what exists today; it does not change any wording.

For the three key-material refusals below — a missing seed, a seed others can read, and an
undecodable one — `prikk key status` reports the same state without needing a commit to provoke it,
and exits `0` while doing so. It names the file in effect, whether it is usable, and why not. See
[Can I Sign Here?](security-setup.md#can-i-sign-here).

## `error: author signing is required: no seed at <path>`

prikk looked for your AUTHOR seed and did not find it. Every commit produces a signed Patch; there is
no unsigned path. The message names the exact file and three ways to get one — create it with
`prikk key generate --out <that path>`, run `prikk setup` in a new project directory, or point
`PRIKK_AUTHOR_SEED_FILE` at a seed file you already have. `seal` says the same for the MAINTAINER
seed. See [First Run](first-run.md) for where the key directory is on each platform.

`prikk key status --role author` shows which of the two inputs it is resolving and to which path —
useful when the file you created is not the file it is looking for. An override that is set and
points at nothing reports `reason: override-missing`, which distinguishes "you have no key" from
"your `PRIKK_AUTHOR_SEED_FILE` is wrong".

prikk 0.40 also reported `error: precondition not met: PRIKK_AUTHOR_SEED is no longer read` when a
seed was still exported as an environment variable. That refusal was deliberately one release wide
and is gone as of 0.41: a stale `PRIKK_AUTHOR_SEED` or `PRIKK_MAINTAINER_SEED` is now simply unread,
and `prikk key status` is where you check which key is in effect instead.

## `error: <path> is readable by group or other (mode 0644); run `chmod 600 <path>``

A seed file anyone but you can read is refused before signing. Run the `chmod` the message names.
(Unix only — on Windows the key directory relies on `%APPDATA%`'s per-user ACL instead.)
`prikk key status` reports the same file as `usable: false` with the mode in its `reason`, and
running it again is how you confirm the `chmod` took.

## `error: <path> must be 64 hex characters, got 8`

A seed file must hold exactly 64 lowercase hex characters (32 raw bytes) — the number after `got`
reports how many your file actually had, so it will differ from the `8` shown here. A shorter,
longer, or non-hex value is rejected before anything is signed; nothing is written to the repository
when this fires. `prikk key status` reports such a file as `reason: undecodable`.

## `error: invalid name: worktree has no node-addressed changes to commit`

You ran `commit` with nothing changed since the last commit or seal — no new, edited, or deleted
files for Prikk to record. Change something first.

## `error: maintainer signing is required: no seed at <path>`

`seal` needs a *maintainer* key, distinct from the *author* key `commit` used — see
[why the tutorial needs a second key](tutorial.md#sealing--and-the-second-key). Same three routes as
the AUTHOR case above, with `maintainer.seed` in place of `author.seed`.

## `error: precondition not met: no maintainer key is adopted in this repository yet`

Your maintainer key is configured but not yet trusted by this repository. Run
`prikk trust maintainer add --key-id ID --public-key HEX` with your maintainer key's id and public half
(`prikk key status --role maintainer` reports the id; `prikk key public --role maintainer` derives the
public key), then seal again. This is a repository-local, trust-on-first-use step — every fresh repository
needs it once, even with a key you have used elsewhere.

The message's parenthetical names the one other way this refusal can be reached: a trust policy
container damaged so badly it replays as empty is indistinguishable, at this read, from one never
written. If you know a key *was* adopted here before, run `prikk doctor`.

Earlier releases reported this same refusal as `error: integrity error: publication trust policy is
missing or unreadable`, which sent readers looking for corruption that was not there.

## `error: precondition not met: <path>: worktree symlink authoring is out of scope`

`commit` found a symlink where it needs a regular file — an untracked symlink in the worktree, or a
tracked file that has been replaced by one, including a **dangling** symlink whose target does not
exist. Symlink authoring is not implemented yet,
so the commit refuses rather than authoring something it cannot faithfully replay. Replace the
symlink with a regular file, or move it outside the repository (or add it to `.prikkignore` if it is
not meant to be tracked), then commit again.

You do not have to run `commit` to find these. `prikk worktree-status` reports the same paths, with
the same reason, under `refused paths:` — see [Worktree Status](worktree-status.md).

Earlier releases reported this as `error: integrity error: worktree authoring: unsupported symlink
authoring: ...`, which sent readers looking for repository corruption. Nothing is damaged.

## `error: precondition not met: <path>: existing TextFile cannot accept non-UTF-8 content`

A path tracked as a text node now holds bytes that are not valid UTF-8. Changing a node's kind from
text to binary (or back) is out of scope, so the commit refuses rather than silently changing what
the node is. Restore valid UTF-8 content at that path, or delete the path and add it again as a new
node, then commit again.

Earlier releases reported this as `error: integrity error: worktree authoring: unsupported kind
transition: ...`. Same refusal, correct class: it is a state you fix, not damage.

## `error: precondition not met: --target names a tag (tags/<t>); pass the block id it points at, or a branch ref`

You ran `prikk tag create --target` against another tag. A tag ref is one hop from its Block — ref →
tag object → block — and a tag of a tag is outside that model, so Prikk refuses rather than quietly
tagging the Block instead: that would make "tag this tag" and "tag this block" produce identical
history. Pass the block id (`prikk tag list` shows each tag's target block) or a branch ref.

`prikk branch create --from` is the other way round: it *does* dereference a tag, because `--from`
means "at the block this ref names".

Earlier releases reported this as `error: object type mismatch: expected block, got tag`, which read
as repository damage rather than as a correctable argument.

## `error: precondition not met: <path> already holds a repository`

You ran `prikk setup` on a directory that already has a `.prikk` in it. `setup` is the first-run
command: it initializes a repository *and* mints a fresh key pair, so there is nothing sensible for
it to do on a repository that already has adopted keys. It refuses before doing anything — no
`init`, no keys minted, no seed file written, even when you passed `--author-seed-out` or
`--maintainer-seed-out`.

Two ways on, both named in the message: to work in the existing repository with keys you already
hold, run `prikk trust maintainer add --key-id ID --public-key HEX` (`prikk key status --role
maintainer` reports the id, and `prikk key public --role maintainer` derives the public half); to start a new project, point `setup` at a different
directory.

Earlier releases got further before failing: they printed `initialized Prikk repository at …`, minted
two keys, wrote any `--*-seed-out` file, and only then reported `maintainer key id maintainer is
already adopted with a different public key`. If you hit that version, the seed files it left behind
belong to keys **no repository adopted** — they are not your keys, and can be deleted.

## `error: integrity error: index entry for <id> resolves to an envelope with computed id <other>`

The object index points at the wrong record: the entry claims an object lives at a location that
actually holds a different one. Your objects are intact — the containers are self-describing and
carry every byte needed — it is only the index that is wrong. Run

```sh
prikk doctor --repair-index
```

which rebuilds the index by scanning the containers, touches no container bytes, and reports how many
entries it moved. Then `prikk verify` again.

This happened when two commands that both write objects ran at the same moment — for example a
`prikk commit` and a `prikk tag create`. Since prikk 0.40 that is prevented: the second command is
refused with `lock conflict` instead, so a repository cannot reach this state any more. The repair
exists for repositories damaged before that.

## `warning [PRIKK-DOCTOR-WORKTREE-RECORD-IGNORED]: .prikk/worktree.baseline is absent or unreadable …` (0.57.0)

`.prikk/worktree.baseline` records which state this worktree was last made to match, and it is what lets
`commit` refuse a worktree that is behind its branch. When it is absent, empty, unreadable, or names a
Block this repository does not hold, it is ignored and that check is off until the next `commit`, `seal`
or `branch switch` rewrites it. Run the command the warning names, `prikk branch switch <current
branch>`: when the worktree matches the branch it writes the record, prints `recorded the worktree's
baseline for <ref>` beside `already on <ref>`, and changes no file. `doctor` shows the warning only when
that command would do so. If you have uncommitted changes the warning is not shown and the command
writes nothing: commit or restore them first (`prikk worktree-status` lists them), and `commit`'s own
refusals protect the worktree meanwhile.

## The way out of a failing finding (0.57.0)

`prikk verify` prints, under what it fails on, a line `way out [<finding>]: …`, and `prikk doctor` carries the
same sentence as the recommendation of the same finding — both read it from one function, so they cannot
disagree. A sentence is always one of three things:

1. **A command that clears the finding** — typed against the damage, after which `verify` exits `0`:
   - an acknowledged commit that is damaged or gone, or a queue tail nothing can explain: `prikk doctor
     --discard-damaged-commits --plan-only`, then without `--plan-only` (it saves what it removes to the recovery
     log);
   - queue metadata that is missing, malformed, or names an owner the acknowledgment history does not recognize:
     `prikk doctor --restore-queue-target --ref <the branch the queued commits belong to>`;
   - a damaged ref pointer index: `prikk doctor --rebuild-pointer-index` (the ref log decides). **One damage can
     fail several things at once:** a damaged block container also fails the ref files whose records name that
     block, so `verify` prints two lines. The rebuild is the way out of the *ref file* only when the pointer
     index is what is wrong; when it says `nothing written` and the finding stays, the ref names an object
     that is damaged or missing, and the stored-object line is the one that applies;
   - a derived cache that disagrees with its source: `prikk doctor --rebuild-caches`;
   - a received ref that failed: `prikk received remove <ref>`.
2. **The sound-copy sequence**, for damage no command repairs in place (a damaged stored object or block, a
   damaged ref log or other appended file, a false merge baseline, a substituted or out-of-order queue record).
   Make it from a backup taken while the repository was sound (`prikk archive export <backup>`):

   ```text
   prikk setup <new-dir>
   cd <new-dir>
   prikk archive verify <backup>
   prikk archive import <backup> --adopt
   prikk verify
   ```

   `setup` makes `<new-dir>` a repository and adopts this installation's maintainer key; the key that sealed
   the history must be adopted there too, which `archive verify` reports. `archive import --adopt` makes the
   backup's branch a local branch and writes its files (`prikk bundle import --input <bundle>` does the same for
   one branch, as a received ref). **Only sealed history travels in a backup:** a commit still waiting for a
   `prikk seal` is made again in `<new-dir>`.
3. **The honest sentence** that no command repairs it in place, followed by (2) — and where a command that
   looks relevant is known to refuse, the sentence says so (`--rebuild-pointer-index` over a damaged ref log,
   `--discard-damaged-commits` over an earlier damaged record or an out-of-order queue, `--repair-tails` over
   interior damage).

## `doctor` reports a finding `verify` fails on (0.57.0)

`prikk verify` exits non-zero on eleven conditions, and `prikk doctor` reports every one of them. The
five it used to pass over, with what to do:

- `warning [PRIKK-DOCTOR-RECEIVED-REF-DAMAGED]` — a `remotes/…` pointer failed verification. It is what
  another repository sent, not local history: `prikk received remove <ref> --plan-only`, then
  `prikk received remove <ref>`; receive it again if you still want it.
- `warning [PRIKK-DOCTOR-LIFECYCLE-CACHE-DIVERGENCE]` — a derived cache disagrees with a replay. Nothing
  recorded is wrong, and `doctor` keeps exit `0`, but `commit` trusts this cache for an incremental step and
  replaces it with a full replay only every 64 steps, so **run `prikk doctor --rebuild-caches` before your
  next commit** (`--plan-only` first): it replays the current branch's lineage from scratch and writes the
  cache, under the lock `commit` takes, and `verify` is clean afterwards.
- `error [PRIKK-DOCTOR-MERGE-BASELINE-DIVERGENCE]` — a merge block's signed baseline claim is false. No
  command rewrites signed history; restore a sound copy (the full sequence is under
  [The way out of a failing finding](#the-way-out-of-a-failing-finding-0570)).
- `error [PRIKK-DOCTOR-ACTIVE-WAL-ORDERING]` and
  `error [PRIKK-DOCTOR-COMMIT-WITNESS-SUBSTITUTED-EARLIER-RECORD]` — the active WAL's records are out of
  order, or an earlier record was substituted. Each is scoped to the `default` session, so it refuses
  that session's repair and no other's. Sealed history is not affected; the queued commits are, and only
  sealed history travels in a backup.

## `error: commit-index cache disagrees with the worktree for at least one path`

`prikk verify` exits `1` with this line, and `prikk doctor` prints
`warning [PRIKK-DOCTOR-COMMIT-INDEX-DIVERGENCE]`, when an entry of the commit-index
(`.prikk/cache/commit-index.v1`) says a file has one content hash while the file's bytes produce
another, although the entry's size and modification time still match the file. The index is a cache
that only saves `commit` a read; it holds no history, so nothing recorded in the repository is wrong.

Run `prikk doctor --rebuild-caches` (`--plan-only` first to see what it reads): it reads every file
`commit` would scan once and writes a correct index, under the lock `commit` takes (and rebuilds the
lifecycle-state cache too, see below), and `verify` is clean afterwards. Two shapes, both cured by it:

- **The entry says the file differs from the last committed content, but it does not** (a damaged
  entry). `prikk commit` re-reads the path, corrects the entry and authors nothing for that file — an
  unrelated new file commits as usual — so this shape cures itself at the next commit. Before 0.57.0 it
  refused the commit with `integrity error: EditText requested for unchanged text`.
- **The entry says the file is unchanged, but it was rewritten** with the same size and modification
  time (a restored or backdated timestamp). `prikk diff` and `prikk worktree-status` show the change;
  `prikk commit` does not, because it trusts the entry — until the rebuild, after which the next
  `commit` authors the edit.

## `error: integrity error: active WAL has a damaged record (damaged record at byte offset N; K sound record(s) follow it); …`

A record in the queue of unsealed commits is damaged, and **K intact records stand behind it**. `prikk verify` and `prikk doctor` name
it and exit non-zero. `prikk doctor --repair-wal-tail` will **refuse** and leave the file exactly as it is: that switch truncates only a
*torn tail*, the incomplete last record left by an interrupted commit, and this is not one (a sound record follows the damage). Keep the
repository as it is and copy `.prikk/active/` aside before doing anything else: the intact records can still be read. A torn tail, by
contrast, shows as `trailing partial WAL bytes: N` and a `PRIKK-DOCTOR-WAL-TRAILING-PARTIAL` warning, and `--repair-wal-tail` is the
right answer to it. (Before 0.48.0 a damaged length was mistaken for a torn tail and the repair deleted the intact records after it.)

## `error: ... the bytes after byte offset N look like many frame headers; …`

**0.49.0 (RFC 167).** A torn or invalid frame's own resynchronisation scan — checking whether a sound
frame is hidden somewhere later in the file, before concluding this is an ordinary crash tail — gave up
after reading 8 times the file's own size without finishing (the message's own words: this binary
"stopped checking ... and treats this as damage, not a torn tail"). This is not a torn tail: the scan
could not rule one out, and ambiguity always resolves to damage, never silently to a tail. In practice
this needs a file deliberately packed with fake frame headers; an honest crash does not produce it
(random bytes following an accidental magic match almost never also claim a plausible length).

A scan this long is damage, not a torn tail, and the tail repairs refuse damage. For a WAL, `prikk doctor` reports
a failed record (`PRIKK-DOCTOR-VERIFY-WAL-RECORD-INCOMPLETE`) and, since 0.57.0, says what is true: `prikk doctor
--discard-damaged-commits --plan-only` shows whether the damaged record is an acknowledged commit it can remove or
declare lost, and when it refuses no command repairs the queue in place, so restore a sound copy (see
[The way out of a failing finding](#the-way-out-of-a-failing-finding-0570)). For the other files, the diagnosis is again `prikk doctor`:

- the pointer index: diagnose with `prikk doctor`; `--repair-pointer-index-tail` refuses damage;
- trust policy or the received index: diagnose with `prikk doctor`; `--repair-tails` refuses damage;
- an object container or the ref log container: **no automated repair.** If `doctor` also reports it as an
  unreferenced remnant, nothing needs the frame and no action is required; otherwise the way out is a copy
  of a sound repository, the same as any other damaged record in these two files.

## `error: N queued commit(s) have lost the record of which branch they belong to`

**Affects 0.20.0 through 0.48.0** (the stranding itself); **0.49.0 adds the way out.** A crash during a
commit *after the first one in a session* could leave `.prikk/active/default/ref-name` empty while the
queue of unsealed commits (the active WAL) still held one or more records — because every commit, not
only the first, used to rewrite this file by a durable truncate then a durable append, and a crash
landing between the two left it empty. 0.49.0 also closes the same condition reached a second way: the
file present but naming a *different* ref than the session's own commit witness does (RFC 166 D6) — both
read identically, since a session cannot have two owners. Either way, the commit itself is not lost (its
bytes are durably queued), but nothing can tell which ref it belongs to, so the way through is blocked:

- `prikk status` shows `queued patches: N targeting <missing metadata>`, with a warning naming the exact
  text below;
- `prikk verify` and `prikk doctor` both exit `1`;
- `prikk commit`, `prikk seal` and `prikk rollback-draft` all refuse with the same text, naming a
  concrete command to run. When this session's own commit record (its "witness") is still readable, it
  names the branch *that* record remembers:

  ```
  error: 2 queued commits have lost the record of which branch they belong to. This session's own
  commit record names heads/main. Check with: `prikk doctor --restore-queue-target --ref heads/main
  --plan-only`
  ```

  Without one (the record itself is gone, or this is a session a pre-0.49.0 binary left behind), it
  names your current branch instead, as a plain assumption:

  ```
  error: 1 queued commit has lost the record of which branch it belongs to. Your current branch is
  heads/main. Check with: `prikk doctor --restore-queue-target --ref heads/main --plan-only`
  ```

  (`commit`'s own text is prefixed `integrity error:`; the others are not, otherwise identical.)
- `prikk doctor --repair-wal-tail` and `prikk doctor --repair-tails` do not help: there is no torn tail
  here for either of them to repair.

**The ref always comes from you** — `--restore-queue-target` never reads it from the witness, even when
the refusal shows one as a hint. **With a readable commit record, `--ref` must match it** — a mismatch
refuses, rather than silently preferring one source over the other. **Without one, `--ref` must be your
current branch** (`prikk status`'s own `current branch: <ref>` line, which reads a different, unaffected
file and so stays right even while `ref-name` itself is empty or disagrees) **unless you pass
`--not-current-branch`** — for example, if you made these commits with `commit --ref <other>` while
sitting on a different branch. A current branch that cannot be resolved at all needs the same flag,
fail closed, since there is then nothing to compare `--ref` against.

**The way out**, run end to end against a real killed repository:

```sh
# If a previous run was interrupted mid-write, its lock may still be on disk. `prikk unlock` lists it
# with the recorded process id; clear it only once you have confirmed that process is gone:
prikk unlock --lock .prikk/active/default/active.lock --yes

# See what this would do first, without writing anything -- the plan shows each queued commit's own
# message and the paths it touches, and the target branch's latest sealed commit, never a bare hash:
prikk doctor --restore-queue-target --ref heads/main --plan-only

# Then do it:
prikk doctor --restore-queue-target --ref heads/main

prikk verify   # now exits 0
prikk seal --allow-no-audit   # the queued commit(s) seal normally, as the restore's own output says
```

```
$ prikk doctor --restore-queue-target --ref heads/main --plan-only
doctor repository: /path/to/.prikk
restoring 2 queued commits to heads/main:
  1. add a (a.txt)
  2. add b (b.txt)
heads/main has never been published -- these would be its first commits
plan only -- nothing written

$ prikk doctor --restore-queue-target --ref heads/main
doctor repository: /path/to/.prikk
restoring 2 queued commits to heads/main:
  1. add a (a.txt)
  2. add b (b.txt)
heads/main has never been published -- these would be its first commits
the 2 queued commits now belong to heads/main; publish them with `prikk seal --allow-no-audit`
```

When there is no commit record to defer to, the plan says so once, honestly, rather than guessing
silently: "prikk cannot tell which branch these commits were made on; your current branch is assumed.
If you made them with `--ref`, restore to that branch."

**Restoring to a branch other than your current one** needs `--not-current-branch`, and the refusal
without it names both branches:

```
$ prikk doctor --restore-queue-target --ref heads/other
error: your current branch is heads/main, but you asked to restore to heads/other; if these commits
were made with `commit --ref heads/other` (or `rollback-draft` on it), pass --not-current-branch to
confirm that; otherwise restore to heads/main instead
```

Nothing about the queued commits' own content is at risk at any point in this sequence — only the small
metadata file naming which ref owns it, rewritten in place (truncated, then appended). 0.49.0 also writes this file once
per session instead of once per commit, closing the window for every commit going forward; see [current
limitations](../reference/current-state.md) for the status of that fix.

**A restored owner is final for this verb**: once ownership is present, a second restore refuses the
same way a healthy session does, even to a different ref and even with `--not-current-branch` — a wrong
restore is undone by `prikk doctor --recovery-restore <run id>`, the run its output names, not by a second call.

## `error: a queued commit you were told had succeeded disagrees with the WAL in a way the WAL's own sound prefix cannot explain (RFC 166)`

**Affects every prikk before 0.49.0, which could not report this at all.** A single flipped byte in the
body of a queued commit's own last frame reads, by itself, exactly like an interrupted write: the
checksum it carries no longer matches, and nothing past it decodes either, so the reader cannot tell "a
process was killed mid-write" from "this byte changed after the commit already finished." Before 0.49.0
that ambiguity resolved in favor of the crash reading — `doctor --repair-wal-tail` treated it as a torn
tail and deleted it, silently, even though the commit had already succeeded.

0.49.0 keeps a small acknowledgment record (one per active session, `.prikk/active/<name>/witness`) and
checks it against the queue before answering. When the queue it names no longer matches, it says so
instead of guessing:

```
$ prikk verify
...
trailing partial WAL bytes: 713
...
acknowledged commits: sequence 1 is damaged
...
error: a queued commit you were told had succeeded disagrees with the WAL in a way the WAL's own sound
prefix cannot explain (RFC 166)
```

(exit `1`; before 0.49.0 this exited `0`, with `trailing partial WAL bytes: 713` the only hint, read
as a harmless tail.)

```
$ prikk doctor
...
error [PRIKK-DOCTOR-COMMIT-WITNESS-ACKNOWLEDGED-DAMAGE]: a queued commit you were told had succeeded
(sequence 1) is damaged
  recommendation: run `prikk doctor --discard-damaged-commits` to remove it; it was already
  acknowledged, so it cannot be removed as a crash leftover
...
error: doctor found repository health errors
```

```
$ prikk doctor --repair-wal-tail
...
active session "default": skipped -- this active session has its own blocking issue: a queued commit
you were told had succeeded (sequence 1) is damaged
...
error: doctor repair skipped one or more active sessions; see the per-active outcomes above for which
and why
```

**The way out**, run end to end against a real killed repository:

```sh
# See what this would remove first, without writing anything:
prikk doctor --discard-damaged-commits --plan-only

# Then do it -- the removed bytes are saved to the recovery log first, all-or-nothing:
prikk doctor --discard-damaged-commits
```

```
$ prikk doctor --discard-damaged-commits --plan-only
doctor repository: /path/to/.prikk
acknowledged commit at sequence 1 (patch 01157f0c...)
340 bytes saved to recovery/log (planned, nothing written) before truncation
plan only -- nothing written

$ prikk doctor --discard-damaged-commits
doctor repository: /path/to/.prikk
acknowledged commit at sequence 1 (patch 01157f0c...)
340 bytes saved to recovery/log (planned, nothing written) before truncation
damaged commit discarded
```

After it, `prikk verify` exits 0 again. The removed bytes are kept until `prikk doctor --recovery-clear` removes them.
They are saved under the run the output names. `prikk doctor --recovery-list` groups the saved entries by run, and
`prikk doctor --recovery-restore <run id>` undoes the whole discard when its conditions hold: the WAL and the commit
witness come back byte-for-byte, and the run's files are checked first, so a later repair that changed one of them is
named (`restore run <later id> first`). The content is read back by restoring the run (the queued commit's own
content, not just its presence, since the WAL body is the signed Patch envelope itself). Rows 5 (the record no longer present at all) and 7 (the acknowledgment history
itself unreadable) are the same verb's job too, with no sequence or Patch id to name in row 7's case —
the plan still says so, honestly, rather than guessing one.

**If the acknowledgment record itself is what's damaged, not the queue:** `doctor` reports a warning
(`PRIKK-DOCTOR-COMMIT-WITNESS-DAMAGED`) instead of an error, and does not refuse — nothing acknowledged
is at risk when the queue itself is sound. `doctor --repair-tails` rebuilds it from the sound queue.

## `error: integrity error: the ref pointer index has an incomplete tail at byte offset N (M byte(s) follow); …`

`seal`, `branch create`, `tag create` and `merge` each refuse **before appending to the pointer index
itself** — the file this refusal protects is always untouched by it. From 0.49.0 (RFC 164 Rule D) they
also refuse before writing any object of their own: `seal`, `tag create` and `merge` run this check at
the start of the command, before their patch, block or tag object is written, and `branch create` writes
no content object before its own pointer-index append. A refusal therefore leaves nothing new behind.
(A refused attempt made by a binary older than 0.49.0 may have left ordinary, content-addressed objects
that nothing references; `verify` still exits 0, and a retry after the repair below reuses them.)

**`prikk compact --pointer-index` refuses the identical way**, before touching anything (RFC 164 round
2, carried to 0.49.0 release prep): the live slot it is about to read and reduce ending in a torn tail
is the same unclean-tail refusal, the same advice, just reached from a different writer — `--plan-only`
is unaffected, since a plan-only run never writes behind the tail it would otherwise refuse over.

This file also has its own narrower `doctor` verb, which repairs only this file. The entries below use
`prikk doctor --repair-tails`, which covers this file too (`args.rs:101`):

```sh
prikk doctor --repair-pointer-index-tail
```

which truncates the incomplete trailing record under the pointer-index lock, saving the removed bytes to
`recovery/log`, the same way `--repair-wal-tail` does for the WAL. After it, `prikk verify` should exit
0, and the publication that refused can be retried.

## `error: integrity error: the trust key container has an incomplete tail at byte offset N (M byte(s) follow); …`

`trust maintainer add` refuses before writing anything, but **only when adding this key would actually
append to the file** — a new key id, or an existing one under a different (conflicting) public key. Both
have to write to `.prikk/trust/keys.container`, and its own last write was interrupted (a crash
mid-append); this command would otherwise append behind that torn tail, blind. **Re-adding a key id
already adopted with the same public key is unaffected**: it appends nothing, so it succeeds regardless
of the tail. Fixed in 0.49.0 (RFC 164 Rule A): the tail is defined by position here now, the same way it
already was for the WAL and the pointer index — a torn prefix, zeros, or garbage all count, whatever the
shape. Run:

```sh
prikk doctor --repair-tails
```

which truncates the incomplete trailing record under this file's own lock, saving the removed bytes to
`recovery/log` first. After it, `prikk verify` should exit 0, and `trust maintainer add` can be
retried.

## `error: integrity error: the trust policy container has an incomplete tail at byte offset N (M byte(s) follow); …`

The same refusal as the trust-key one above, for the trust-policy container instead
(`.prikk/trust/policy-a.container` or `-b.container`, whichever `prikk verify`'s own report names as
live) — `trust maintainer add` and `trust maintainer remove` both read this container, and both refuse
here **only when they are about to append their own new snapshot**: `add` of a key id not yet in the
policy, or `remove` of one that is (and is not the last one). Re-adding an already-adopted key, or
removing one that was never adopted, appends nothing and is unaffected by a tail here. The way out is
the same: `prikk doctor --repair-tails`, then retry.

## `error: integrity error: the author key container has an incomplete tail at byte offset N (M byte(s) follow); …`

A commit, a rollback draft, or a `bundle import`/`sync accept` refuses here **only for an author key id
this repository has not recorded material for yet** — recording it would append to
`.prikk/trust/author-keys.container`, and its own last write was interrupted (a crash mid-append); the
same shape as the two trust-store refusals above, for the file that records AUTHOR (not MAINTAINER) key
material. **A commit (or import) by an author key id this repository has already recorded material for
is unaffected**: it appends nothing to this file, so it succeeds regardless of the tail. **For a
`commit`, this check runs before any of the commit's own writes** (RFC 164 Rule D, 0.49.0): it sits
ahead of the blob, object-index and commit-index writes, so a refused commit by a new author key leaves
none of them behind. The check runs again immediately before the WAL append that would queue the commit.
(A refusal made by a binary older than 0.49.0 may have left an unreferenced blob and index entry; they
are ordinary content-addressed objects, and a retry after `prikk doctor --repair-tails` reuses them.)

## `error: integrity error: the received index has an incomplete tail at byte offset N (M byte(s) follow); …`

`bundle import` refuses **before writing anything at all** — not only leaving the received-ref index's
own live slot (`.prikk/refs/containers/received-index-a.container` or `-b.container`) untouched, but
also the bundle's own objects and any author-key material it carries: the check runs before the first
object write, the same as every other decision an import makes. (`sync accept` never writes to this
file; a received/`remotes/*` ref is only ever created by `bundle import`.) Run `prikk doctor
--repair-tails`, then retry the import.

## `error: integrity error: the <container>'s generation log has an incomplete tail at byte offset N (M byte(s) follow); …`

`prikk compact` refuses **before writing anything at all** — before the retired slot is truncated, not
only before the new generation record — when the container's own generation log (`<container>` is "ref
pointer index", "received index", or "trust policy container", whichever `compact` flag you ran) ends in
a torn tail from an interrupted `compact`. This is the same rule as the four container refusals above,
one layer up: `compact` already reads the generation log to pick the live slot, and this check rides that
same read rather than performing a new one. **`compact --plan-only` is unaffected**: a plan-only run
writes nothing, so it never refuses over a tail it would never write behind. Run `prikk doctor
--repair-tails`, then retry `compact`.

## `error: integrity error: <container> has a damaged entry; …` (seen for a damaged generation-log *record*, not a slot)

Seen from any command that reads a generation log's live slot — `status`, `log`, `branch list`, `seal`,
`commit`, `verify`, `doctor`, and `compact` itself for that container — for either of two reasons (RFC
164 Rule A, §9 and §9.2): a **sound** record follows damaged bytes further into the file, or the **last**
record is itself *complete* — its checksum verifies, recomputed with this format's own real magic and
version, over either the claimed length or the length to the end of the file — but its envelope still
fails, or its checksum itself is wrong. Neither is a tail: a tail is what a crash leaves, and a crash can
only leave something *incomplete* — a torn prefix, or zeros/garbage with nothing sound after them at the
very end. A complete record was fully written, **whatever its own stored magic, version, or length field
says** (one of those three can itself be the single corrupted byte, with the checksum and body untouched
— §9.2); a failing checksum or envelope on it is corruption after the fact, not a crash, so RFC 164 §9/
§9.2 treats it exactly like interior damage, never as a tail to be repaired away (repairing it away would
silently revert whichever slot it names to the previous one — measured, and closed, on a real build; a
corrupted header field did so even before any repair ran, until §9.2 closed that too).
**On the ref pointer index's own generation log, this stops every command that resolves a ref**, since
every one of them reads it; on the received-index and trust-policy generation logs, only `compact` is
affected. Trailing bytes that do not form a complete record (a torn tail) are truncated by `--repair-tails`,
as for every covered file; the removed bytes are saved to `recovery/log` first. Damage in the middle of the file,
or a complete but corrupt last record, has no repair: `--repair-tails` refuses it, rather than guessing which
bytes are safe to remove, or silently undoing the decision the damaged record carried. **This entry gives no truncation advice**: whether the
damage sits before a sound record or is the complete-but-corrupt last record itself, no offset here is
one a repair can safely remove.

**Handoff 165 Q1b:** `doctor` has nothing of its own for a damaged generation-log *record* (unlike a
damaged slot, generation logs are not among the files `check_appended_file_tails` reports), so the
message names the way out directly instead of sending you to `doctor`: the ref pointer index names the
rebuild (`prikk doctor --rebuild-pointer-index --plan-only`, then the real run — the remedy is the same
as a damaged slot's, since the rebuild reads neither); the received index and the trust policy
container name a copy of this repository's whole `.prikk/` directory from a backup taken before the
damage (the trust policy text also says to re-apply every trust change made since).

## `warning: <container>'s generation log names no live slot; slot <X> was deduced from the entries (…)`

0.50.0 step 1 Part E4, corrected by handoff 165 (019 §5.7; 020 §3.1). Seen from `prikk verify` or
`prikk doctor`, when a compacting container's generation log reads as empty or absent — not damaged,
not a tail, genuinely nothing — while the *other* slot (`b`) holds real data. Slot `b` is never
written except alongside the one generation record that names it live, so this shape can only mean a
compaction (or, for the ref pointer index, `prikk doctor --rebuild-pointer-index`'s own rebuild)
genuinely happened and the record of it was lost afterward — an accidental deletion, or a partial
restore from backup.

**This is a warning, not a refusal, when content *can* decide: every ordinary command already
resolves it correctly.** Content is tested in **both directions** (handoff 165 corrected Part E4's
own rule, which tested only one): slot `a` is stale beside `b` — or `b` stale beside `a` — when
either slot equals, or is a prefix of, a compaction of some earlier prefix of the other's own history
(a crash before a switch was recorded, or a write cut short), or when the other slot starts with a
full compaction of it and simply kept taking writes after. Whichever one direction holds names the
live slot — covering a crash between a compaction's own new-slot write and its generation record, a
partly written slot, and a crash that leaves a slot live but still taking ordinary writes afterward (a
new branch, a revocation), without losing those writes or reinstating what they revoked the way
comparing only against the stale slot's *current* content (an earlier version of this rule) could.
Both directions holding and the two slots folding to the *same* entries means either reading is
correct (reported as "both slots agree"). Only when **neither** direction holds, or both hold while the
folds **disagree**, does the deduction genuinely fail — see the next entry; this is distinct from one
of the two slots being damaged, which refuses too but for an unrelated reason (the container's own
damage text, not this one). A restore from the recovery log refuses outright in this ambiguous-or-
damaged state instead of deducing, since it is a deliberate writer and a stale meaning file could
otherwise compare unchanged and pass.

**The way out, named in the warning itself.** For the received index and the trust policy container:
run the exact command it names — `prikk compact --received-index` or `--trust-policy`. It resolves
the identical way, saves the slot it is about to overwrite and the generation log as one recovery
run first (both saves landing before either file is written), then writes a fresh generation
record, ending the deduced state for good. **For the ref pointer index, `compact --pointer-index`
instead refuses outright here** (Q2 review): the warning names `prikk doctor
--rebuild-pointer-index --plan-only`, then the real run, which re-derives the whole index from the
ref log directly, without reading either slot as live — this is the pointer index's only way out of
this state now, not merely one option among others.

## `error: ambiguous generation log: <container>'s generation log is lost, and its two slots fit two different histories; …`

Handoff 165 Q1b (020 §3.1, Q1 review). Seen from the same commands as the warning above, when
content genuinely **cannot** decide: either neither of the two slots relates to the other at all (not
a shape any real compaction sequence produces), or both relate and their folds disagree. The Q1
review proved this is not a gap in the rule but a real property of the state: two equally honest
command histories — add a key, compact, remove it; or compact again, then add the key back — can
leave *byte-identical* slot files with opposite answers on whether that key is trusted. Once the
generation log that would have told them apart is lost, no rule that reads only the slots can be
right in both histories, so refusing is the only honest answer. This is a distinct, typed error
(`PrikkError::AmbiguousGenerationLog`, not `Integrity`) — nothing here is corrupt. `doctor` raises
`PRIKK-DOCTOR-GENERATION-LOG-AMBIGUOUS`, printed first, before the current-branch warning and any
`PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE` error the same ambiguity also causes.

**The way out, named in the message itself:** for the ref pointer index, run `prikk doctor
--rebuild-pointer-index --plan-only`, then the real run — it re-derives the index from the ref log,
never reading either slot, so the ambiguity does not touch it. For the received index and the trust
policy container, 0.51.0 adds a second way out, named first: `prikk compact --received-index
--keep-slot a|b --plan-only` (or `--trust-policy`) prints each slot's own entry count and folded
state — for the trust policy, the keys trusted in one slot and not the other, by name — and which
slot content would deduce, or "ambiguous: prikk will not choose," before anything is written. The
user knows which history is theirs; running the real command (without `--plan-only`) saves the
slot it overwrites and the generation log, as one recovery run, before writing anything. A whole-
`.prikk/` backup taken before the generation log was lost stays the second way out (the trust
policy text also says to re-apply every trust change made since that backup) — and the only one
left for either container once the generation log is lost *and* the chosen slot does not decode
cleanly (`--keep-slot` refuses on a damaged chosen slot, naming that fact, rather than guessing).

## `error: integrity error: the ref pointer index's live slot is not recorded; run \`prikk doctor --rebuild-pointer-index --plan-only\`, then \`prikk doctor --rebuild-pointer-index\` first`

0.50.0 step 1 Part E3/E4 (019 §5.7), corrected by Q2b (H1 ruling item 2). Seen from `prikk doctor
--recovery-restore` (plan or run), when the entry being restored names a meaning file in the ref
pointer index or the ref log, and the pointer index's own generation log names no live slot
(deduced or ambiguous alike). Unlike an ordinary read, a restore does not deduce here: it is a
deliberate writer, checking that a meaning file's state still matches what the entry recorded
before trusting it, and a stale meaning file in this exact state could compare unchanged and let a
restore through that should not proceed. The message used to name `prikk compact --pointer-index`
as the way out; that command now refuses outright in this exact state, so it would just hand back a
second refusal — the rebuild is named instead, for either sub-case. Run it, as the message says,
then retry the restore.

## `error: integrity error: <container> has a damaged entry; run doctor before reading`

Seen from `trust maintainer add` or a commit, naming the trust-key or author-key container, for
either of two reasons (RFC 164 Rule A, §9 and §9.2): a **sound** record follows damaged bytes further
into the file, or the **last** record is itself *complete* (its checksum verifies against this
format's own real magic and version, whatever its own stored header fields say) but its envelope or
checksum still fails. Neither is a tail — see the generation-log entry above for why a complete
record is never one, whatever its position; a genuine tail (trailing zeros, garbage, or a torn prefix
at the *end* of the file, with nothing sound after it, and not itself a complete record) reads as one
of the four "incomplete tail" entries above instead, repaired by `prikk doctor --repair-tails`.
**`doctor` has nothing that repairs interior damage in either container** — `--repair-tails` refuses
on it, the same way every other covered file's own repair does, rather than guessing which bytes are
safe to remove, or silently reverting the decision (a trust adoption, a revocation) the damaged
record carried. This entry gives no truncation advice: there is no offset here promised to be safe to
remove. If you believe the bytes after some offset really are nothing but trailing garbage from an
interrupted append (not a complete, corrupted record), run `prikk verify` first and read its own
report carefully before deciding to truncate anything by hand; when in doubt, back the file up and
ask before changing it.

## `error: integrity error: ref pointer index has a damaged entry; run \`prikk doctor --rebuild-pointer-index --plan-only\`, then \`prikk doctor --rebuild-pointer-index\``

0.50.0 P3c (019 §2.2, §4.4). Seen from `verify`, `doctor`, `branch list`, or anything that resolves
the current branch, for the identical two reasons the entry above names, but for the ref pointer
index specifically — which, unlike the trust-key and author-key containers, *does* have a way out:
the plan, then the rebuild for real, re-derives the index from the ref log directly, never reading
the damaged slot at all. Before 0.50.0 the message said only "run doctor before reading," which
reads this exact container the exact same way and so answers with the identical failure again — the
rebuild is what actually clears it. `doctor`'s own `PRIKK-DOCTOR-POINTER-INDEX-INTERIOR-DAMAGE`
issue prints first, before the current-branch warning and any `PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE`
error the identical damage also causes — each of those now names this issue instead of the generic
stage-incomplete sentence (which, since 0.57.0, says what is true instead of "inspect the failing stage").

## `error: integrity error: received-ref index has a damaged entry; no repair exists -- preserve the repository; the way out is a copy of this repository's own \`.prikk/\` directory from a backup taken before the damage` (or the identical shape for the trust policy container, which adds re-applying every trust change made since)

0.50.0 P3c (019 §2.2, §4.4). Seen from a `bundle import`, `sync accept`, or `trust maintainer
add`/`remove`, for the identical two reasons as the entry above. Unlike the ref pointer index, there
is no rebuild for either of these: neither container is re-derivable from anything else this
repository holds (the received index is the only record of what was received; the trust policy
container's own history is not reconstructable from the keys alone). The message says this plainly
and names a copy of the **whole** `.prikk/` directory, never a single file, as the way out: the two
slots and the generation log are one set, and restoring the slots alone could leave a log naming a
slot the restored copy disagrees with. For the trust policy container specifically, an older copy
could re-trust a key that was revoked since — the message also says to re-apply every trust change
made since that backup. Both replace the same circular "run doctor before reading" the pointer index
used to share with them, and `doctor`'s own dedicated issue for each prints first, the same as the
pointer index's own.

## `error: integrity error: object <id> references missing <role> <id>`

Seen from `verify` (and anything that calls it, such as `doctor`) after a `bundle import` was
interrupted by a crash partway through, when the object making the dangling reference is itself
reachable from committed state (an unreachable one is a harmless remnant instead — see
`PRIKK-DOCTOR-UNREFERENCED-REMNANT` below, which now uses the same canonical form). Exact wording
varies with what was missing when the crash landed, but the shape is always `object <owner>
references missing <role> <id>`:

- `object <id> references missing snapshot blob <id>`
- `object <id> references missing block patch <id>`
- `object <id> references missing parent block <id>`
- `snapshot of Block <id> names Blob <id> for <path>, which is missing`
- `lifecycle replay: blob <id> required for a state effect is missing`

An unreachable owner producing the identical dangling reference is not an error at all, just a
warning naming the same two objects in the same form:

```text
warning [PRIKK-DOCTOR-UNREFERENCED-REMNANT]: object <owner> references missing <role> <id>
```

In 0.48.0 and earlier, `bundle import` wrote the objects it carried in the bundle's own order, not
in dependency order — across object types, and, within the same type, a child block could be listed
before its own parent — so a crash partway through could leave a Block durable while something it
names is not. **None of `doctor`'s three repairs (`--repair-wal-tail`, `--repair-index`,
`--repair-pointer-index-tail`) clears this** — they do not know this shape. **The way out is to run
the same `bundle import` again**, with the same input: every object it carries is content-addressed,
so the retry only writes what is still missing, and this cleared the state in every case reproduced
(25 of 25). Fixed in 0.49.0: `bundle import` now writes objects in full dependency order (across and
within kinds), so an interrupted write can no longer produce this shape. `sync accept` is ordered the
same way now, for the same reason.

**0.49.0: this error now fires only when the Block making the dangling reference is itself reachable
from committed state** — a ref, a received pointer, a queued patch, or a sealed block reached from
them (RFC 164 Rule E). A Block nothing reaches this way is an **unreferenced remnant**, reported as a
warning instead — see the next entry.

**If this came from `prikk show` on a sealed block, under 0.52.0 or 0.53.0, specifically on the
second (or a later) consecutive `EditText` of the same file** — `lifecycle replay: blob <id>
required for a state effect is missing`, from `show` alone, in either its text or `--format json`
output — this is not the interrupted-import damage the rest of this entry describes. It is a read
bug in `show`'s own sealed-block fold, fixed in 0.54.0 (CHANGELOG, `### Fixed`): the stored history
is sound, `prikk verify` exits `0` the whole time, and no repair is needed or possible for it. The
way out is to upgrade to 0.54.0 or later; running any of this entry's repair steps on a repository
that only has this symptom fixes nothing, because nothing is actually damaged.

## `warning: <owner type> <id> references missing <missing type> <id> (<role>) -- re-run the import if you still have the bundle; otherwise it is harmless`

Seen from `verify` and `doctor` (as `PRIKK-DOCTOR-UNREFERENCED-REMNANT`) for a stored object's own
dangling reference when nothing committed still needs it — not reached by any ref, received pointer,
queued patch, or sealed block reached from them (RFC 164 Rule E, 0.49.0). The same underlying shape as
the previous entry's error, told apart by reachability: a Block left behind by an interrupted `bundle
import`/`sync accept` that nothing in this repository was ever going to use is harmless debris, not
damage, and `verify` exits `0` over it. No command removes a remnant in 0.49.0; if the bundle that
produced it is still available, re-running the same import is still the way to make it whole, exactly
as the previous entry describes — this warning simply means doing so is optional, not required for
`verify` to pass. **A remnant that later becomes reachable** (a new branch created over it, say) is
reclassified as damage — the previous entry's error — on the very next run: reachability is always
recomputed fresh, never cached.

## `error: repository has interrupted or divergent ref publication state`

Seen from `verify` after `branch create`, `tag create`, `sync adopt-tag`, `merge`, or `seal` was
interrupted by a crash partway through publishing a ref: the ref's pointer names a `RefState` the
ref log has not yet confirmed (a "pointer lead"). `--format json` and `doctor` name one of two codes,
and they mean different things.

**`PRIKK-VERIFY-REF-POINTER-LEADS-LOG`** — the lead is **completable** (RFC 165 R4): the leading
`RefState` verifies under the current trust policy, chains cleanly to the log's own tip, names a
target that exists, and, if the interrupted publication consumed the active WAL (`seal`/`sync seal`),
the retained WAL evidence still matches it. `verify`'s own detail line names the way out directly
(019 §5.2: `verify` prints only its own message, never a separate recommendation the way `doctor`'s
issues do), with and without a tail on the ref log container itself:

```text
ref-publication [PRIKK-VERIFY-REF-POINTER-LEADS-LOG]: authoritative pointer leads committed ref log by one transition; run `prikk ref complete <ref>`
ref-publication [PRIKK-VERIFY-REF-POINTER-LEADS-LOG]: authoritative pointer leads ref log by one transition with <N> incomplete trailing byte(s); run `prikk ref complete <ref>`
```

`doctor` recommends the same thing, `prikk ref complete <ref>`. Retrying the same command does not finish it:
`branch create`/`tag create` still answer "already exists" (the ref *was* durably created — unless
the pointer itself also reports a completable lead, in which case the refusal itself now names
`ref complete`), and `merge` refuses before gathering evidence rather than building on a pointer its
own log has not caught up to.

**`PRIKK-VERIFY-REF-DIVERGENCE`** — the lead fails one of those conditions (an untrusted or revoked
signer, a broken chain, a missing or wrong-kind target, mismatched WAL evidence, or damage elsewhere
in the ref log or pointer index). Three distinct shapes land on this one code, each with its own
detail line:

```text
ref-publication [PRIKK-VERIFY-REF-DIVERGENCE]: format-2 ref log leads the authoritative pointer
ref-publication [PRIKK-VERIFY-REF-DIVERGENCE]: format-2 ref pointer is missing while committed log history exists
ref-publication [PRIKK-VERIFY-REF-DIVERGENCE]: ref-log chain or sequence diverges for <ref>
```

The first two are legacy format-2 shapes (format 6/7 repositories cannot produce them through any
normal command — pointer-then-log write order makes them unreachable); the third is a genuinely
broken chain. `doctor` recommends manual recovery only; preserve the repository and ask before
changing anything.

**`prikk ref complete <ref>`** finishes a completable lead: it appends one more signed `RefUpdate`
record to the ref log, matching the state the pointer already carries, through the exact same write
`seal`'s own DC-38 retry uses (never a new code path for the append). **Any adopted maintainer key
may complete it, not only the one that started it** — the leading `RefState` is already signed by an
adopted maintainer; the completer's own key only signs the log record, so a different operator can
finish a crash another operator's command left behind. `--plan-only` prints what it would do (the
leading state, its target, the log sequence it would append at, and which key would sign it) without
writing anything. `seal`'s own retry of the same ref is unaffected — DC-38's natural-retry mechanism
still completes a `seal`-shaped crash through its own existing path, and `ref complete` refuses a ref
that is not actually a pending completion (already caught up, or not published at all) the same way.
Measured for `merge`: 10 of 300 kills on 0.48.0, 16 of 300 on the build carrying 0.49.0's own
dependency-order fix (the two are unrelated defects reached through different commands); every one of
those crash states is now either completable (`ref complete`) or named as a genuine divergence.

A complete damaged ref-log record (never a tail, RFC 164 §9.2) refuses any of `seal`/`branch create`/
`tag create`/`sync adopt-tag`/`merge`/`ref complete`/the rebuild below, identically:

```text
error: integrity error: the ref log has a damaged record at byte offset <N>: <checksum/decode detail>; this is not an incomplete publication and no seal retry resolves it -- the way out is a copy of a sound repository, not a repair
```

### `prikk doctor --rebuild-pointer-index` (RFC 165 R5)

The other way out: re-derives the ref-pointer index from the ref log directly, structural and never
trust-filtered (no signature is re-checked for a record already durable in the log — only a *current*
lead re-enters trust, exactly as above). Where `ref complete` finishes one ref's own pending
transition, the rebuild recovers the whole pointer index when *it* — not the ref log — is what is
damaged or untrustworthy: a flipped byte in a pointer record, a stale fallback behind the log, or a
lead that turns out not to verify at all.

`--plan-only` prints the same plan a real run writes from (K1), per ref: its state before and after,
and every lead it would drop or restore:

```text
heads/main: <before> -> <after>
restored from the log: heads/main (RefState <id> was stale, behind the log)
dropped lead: heads/topic (RefState <id>) -- the leading RefState's signature does not verify under the current trust policy: <detail>
```

**"Restored" and "dropped lead" are never the same claim.** A pointer *behind* the log — commonly a
damaged newest pointer-index record, read as an older, already-log-confirmed entry instead — is
*restored*: nothing authorized is discarded, the ref simply ends where the log already soundly
confirms. A pointer genuinely *ahead* of the log that fails RFC 165 R4's own rule is a *dropped
lead*: a real, signed transition the rebuild refuses to carry forward, because it cannot verify.

It refuses, writing nothing, over the same ref-log damage/tail shown above, or over **any** completable
lead anywhere in the repository (complete it first — overwriting it would drop an authorized
transition, the one thing this verb must never do):

```text
error: precondition not met: <N> ref(s) have a completable lead; run `prikk ref complete <ref>` first -- a rebuild would drop an authorized transition: <ref list>
```

A damaged pointer-index record is **not** a refusal reason — serving past it is the rebuild's own
purpose. Structural, no signing: a history entirely signed by a key later revoked is unaffected (the
rebuild moves no ref on trust grounds alone), but a *lead* signed by a revoked key is dropped, named,
same as any other failed condition.

## `error: precondition not met: checkout target for <ref> is not a checkpoint, so it carries no snapshot …`

The block you asked to check out has no snapshot, which is the normal state of most blocks: `seal`
writes one only at a ref's first block and every 64 blocks after it. Use the patch-replay route the
message names, which does not need a snapshot:

```sh
prikk checkout --patch-plan --ref <ref>
```

Releases before 0.43 reported this as `checkout target for <ref> does not contain a snapshot blob`, and
earlier ones still under `error: integrity error:`, which read as damage. Nothing is damaged, and nothing
was ever missing.

## error: precondition not met: no prikk repository at \<path\>

The directory you ran the command in — or the path you passed — has no `.prikk` directory. Run the
command inside a repository, pass the repository root as the path argument, or create one with
`prikk setup <path>`.

Before 0.45 this was `error: i/o error: No such file or directory (os error 2)`, which did not say which
file, or that the repository itself was absent. A real I/O failure inside a repository is still
`error: i/o error:`.

## `error: precondition not met: ref <ref> does not exist in this repository`

A command that reads or advances a ref was given one that is not there — usually a typo, or a branch not
created yet (`prikk branch list` shows what exists). If the message goes on to say `remotes/<ref>
exists`, the name you gave belongs to a *received* ref: pass it as `remotes/<ref>` to a command that
reads received refs (`log`, `merge-evidence`, `merge-plan`, `bundle preview`), or take it into a local
branch with `prikk merge --from remotes/<ref>`.

Before 0.45, `prikk log --ref` on an absent ref reported an empty history with exit 0 and `prikk
worktree-status --ref` reported changes; other commands said `ref <ref> is not published` (as `error:
integrity error:` or `error: invalid name:`), `does not exist, nothing to export`, `does not resolve to a
published ref`, or a false "not a checkpoint". Scripts that treated an empty `log` as "no history yet" now
see exit 1.

**This refusal never applies to the repository's own current branch** (`prikk status` names it),
published or not: `log`, `worktree-status`, `tree`, `cat --path`, a bare `diff`/`diff --from` (the
worktree's baseline), and `checkout --plan-only` answer for it exactly the same way whether it is named
with `--ref`/`--from` or left off. 0.45.0 and 0.46.0 refused a fresh repository's own current branch when
it was named explicitly — the one ref this message was never meant to name — which is fixed in 0.47.0;
see the CHANGELOG. Every *other* name, published or not, still refuses exactly as this section describes
— including the current branch named as one side of `diff --from … --to …`.

## `error: precondition not met: remotes/<ref> is a received ref, and this command does not accept received refs; …`

Received refs (`remotes/…`, from `prikk bundle import` or `prikk sync accept`) are untrusted pointers:
they can be read and merged from, but `checkout`, `inverse-plan`, `rollback-preview`, `bundle export`,
`branch create --from` and `tag create --target` do not accept them. The rest of the message names the commands that do read them. To work on the received
history, merge it into a local branch with `prikk merge --from remotes/<ref>`.

## `error: integrity error: existing container record for <id> differs from candidate -- this repository is format 6, …`

Someone sent you an object you already hold, carrying signatures yours does not — a second signer of the
same patch or block. A format-6 repository holds one record per object, so it cannot keep both; format
7 merges the signatures into one object. Check that every prikk that opens this repository is 0.45 or
newer (older binaries refuse a format-7 or format-8 repository at open; the upgrade moves the repository to format 8, which 0.56.0 and earlier refuse), then:

```sh
prikk format upgrade
```

and repeat the import or accept. `format upgrade` verifies the repository first and changes only the
`FORMAT` marker; running it on a format-8 repository changes nothing.

## `error: precondition not met: this bundle declares repository format 8, and this repository is format 7; …`

A bundle or an archive (the message says which) was exported from a **format-8** repository, which may hold non-ASCII paths, and
this repository is format 6 or 7, which cannot. Nothing was written. A new repository is created at format 8, so an older one
that receives work from a newer one meets this. The message names the way forward, and it is the same one `commit` gives for a
non-ASCII path:

```sh
prikk format upgrade
```

then repeat the import. `format upgrade` verifies the repository first and changes only the `FORMAT` marker, and there is no
downgrade: a prikk older than this one (0.56.0 and earlier) cannot open the repository afterwards. Every artifact from a format-8
repository declares 8, even one with only ASCII paths, because the declaration is the repository's. `bundle verify` and `archive
verify` need no repository and still pass; `bundle preview` gives the same refusal as `bundle import`. Under `--format json`, the
three import and preview commands print a `refusal-v1` document with `"refusal": "repository-format-too-old"`.

## `error: precondition not met: <path>: this name is not in Unicode normalization form C (NFC) …`

`commit` (and `worktree-status`, as `authoring: refused`) found a file whose name is not in NFC: an accented letter spelled as a
base letter plus a combining mark (`e` then U+0301) instead of the single precomposed character (`é`, U+00E9). The two look
identical on screen, which is why the message writes the non-ASCII characters as `\u{..}` and gives both spellings. Another tool made
the name (macOS writes names this way, and some archive tools do), and prikk records only NFC names and never renames a file for you.
Rename the file to the NFC spelling the message gives, then commit again. If a file with the NFC spelling already exists beside it, the
message says the two are one visible name in two spellings: keep one and rename or remove the other.

## `error: invalid name: this version … does not accept U+200D (invisible format character) in a repository path: …`

A file name contains a character no repository path may contain, in any format: a control character, a noncharacter, a line or
paragraph separator, a bidirectional control, or an invisible format character such as the zero width joiner (U+200D) or non-joiner
(U+200C). The message names the character and the file. Rename the file without it; no `prikk format upgrade` helps, and none is
suggested. The reason these are refused is that two names differing only by an invisible character would look identical. ZWJ and ZWNJ are
needed to spell some scripts and emoji; they are refused on purpose for now, and that is revisited if a consumer asks. The same
character in history written by another version is reported as outside what this version accepts, not as damage.

## `error: invalid name: case-insensitive path collision involving: <path> and <path> …`

`seal` (or `checkout`, `branch switch`, `extract`) found two paths that are the same name once Unicode case folding and normalization
are applied, so a case-insensitive or normalizing file system (macOS, Windows) would make them one file. The check is deliberately
strict: `straße` and `strasse`, `É` and `é`, and an NFC and an NFD spelling of one name all collide, even where your file system keeps
them apart. The message names both paths, with non-ASCII characters written `\u{..}`. Nothing was sealed or written. Rename or remove one
of the two files (for a checkout, the history itself holds both, and was sealed by another version), then try again. `commit` records
both without complaint; the refusal comes at `seal`, as it always has for case collisions.

## `error: bundle declares repository format 9, which this build does not know how to read; … so this is not damage …`

A bundle or an archive (the message says which) names a repository format this prikk does not know. This is not damage: the file's
checksum was sound, and a different, newer prikk wrote it. (An archive's message used to begin `malformed persisted data:`, which
read as damage.) `archive verify` always refused an unknown number; the bundle's was only informational before 0.57.0. Use a prikk
that knows the format, or export again from a repository this prikk can read. Under `--format json`, `bundle import`, `bundle
preview` and `archive import` print a `refusal-v1` document with `"refusal": "outside-version-rule"`.

## `warning: patch <id>: a path in it is outside what this version …; nothing this archive carries reaches it …`

`archive verify` found a patch in the backup whose path this version's rule refuses (a newer prikk, or 0.55.0, let it into the
repository), and no ref the archive carries reaches it, so it is held as an unreached object, as the repository's own `verify` holds
it. This is a warning, not a failure: the archive is internally consistent, `archive import` accepts it, and the receiving
repository's `verify` passes. If a carried ref **does** reach the patch, `archive verify` and `archive import` refuse the archive,
naming the path: read that history with a newer prikk.

## `error: precondition not met: <type> <id> would carry at least 5 counted signatures, above the limit of 4 per object …`

An import or accept would put more than four counted signatures on one object. The operation is refused
whole and nothing is written. A MAINTAINER signature by a key adopted in this repository is not counted,
so the limit only bites on signatures this repository has no reason to trust. Ask the sender which
signatures the object is meant to carry.

## `error: author signing refused: <path> holds key id <id>, but the seed at <path> derives <other> …`

The key-id file beside your seed (`author.key-id` or `maintainer.key-id`) names a different key than the
seed derives: the seed was replaced, or the file was copied or edited. Restore the seed the file belongs
to, or remove the file so the id is derived again. A custom key id is set with `PRIKK_AUTHOR_KEY_ID` or
`PRIKK_MAINTAINER_KEY_ID`, never by editing the file. `prikk key status` reports this state as
`reason: key-id-file-mismatch`; the same refusal reads `maintainer signing refused:` for the maintainer
key.

## `error: precondition not met: the worktree was materialized from the snapshot of Block <block> on <ref> and is not replay-verified; run `prikk verify` …`

You ran `checkout --snapshot-materialize`, and the worktree it wrote is the block's signed state rather
than a replay of its history. Until the repository has been replayed, `commit`, `mv`, `seal`, `merge`,
`sync accept`, `sync seal`, `rollback-draft --append-inverse` and `branch switch` refuse, so nothing
derived from an unverified worktree becomes history. Run:

```sh
prikk verify
```

When verify finds nothing about objects, the WAL, refs or block state, it prints `provisional worktree:
replay-verified; marker cleared` and the commands work again. If it prints `provisional worktree: kept`,
fix what verify reported first. See [snapshot materialization](checkout/snapshot-materialization.md).

## `error: precondition not met: <old> -> <new>: the source is present in the worktree again, …`

A `prikk mv` declaration says `<old>` became `<new>`, and the worktree says otherwise — most often
because the move was undone with a shell `mv` rather than with `prikk mv`. The commit refuses rather
than guess which one is the tracked node, and **the worktree still reads `clean`**: nothing is
missing, modified or untracked, so `worktree-status` reports it as `refused declarations: 1` instead.

The refusal names the way out for the state you are actually in, and each command was measured:

```sh
prikk mv <new> <old>    # drop the declaration: it nets to no move and is removed
prikk mv <old> <new>    # or make the move again, and commit authors the rename
```

A shell `mv` back is not one of them: it recreates the state that produced this refusal.

If **both paths exist**, `prikk mv` refuses in either direction until one copy is set aside — delete
`<old>` and commit to author the rename, or delete `<new>` and then run `prikk mv <new> <old>`. If the
destination is **another tracked node** this commit does not also move or delete, `prikk mv <new> <old>`
drops the declaration; `<old>` keeps its content and `<new>` stays deleted, so the commit authors that
deletion. See [Declared Moves](patches/declared-move.md) and [Worktree Status](worktree-status.md).

## `warning: the snapshot of Block <block> failed validation (…); this report replayed the whole history instead -- run `prikk verify``

`checkout --patch-plan`, `--patch-delete-plan` or `bundle preview` tried to start at a snapshot, and the
snapshot did not validate. The report did not use it: it replayed the whole history, and its output and
exit code are what they would have been with no snapshot. The warning is a sign of a damaged or
tampered snapshot object. Run `prikk verify`, which names the block and the finding.

## `error: active WAL has no patch records to seal`

You ran `seal` with nothing queued — every commit since the last seal has already been published.
There is nothing to fix; commit something before sealing again.

## `error: invalid name: ref topic is not a local branch ref; expected heads/<name>`

You passed a bare name (`topic`) where Prikk expects a fully-qualified ref
(`heads/topic`). `branch create`, `branch close`, `tag create`, and any `--ref` flag all take the
qualified form — see [why refs are fully qualified](faq.md#why-is-it-headstopic-and-not-topic) for
the reasoning. Re-run the same command with `heads/` (or `tags/` for a tag) in front of the name.

## `error: i/o error: repository mutation requires Linux, macOS, or Windows root-scoped filesystem capabilities`

You are running `init`, `commit`, or `seal` on a platform other than Linux, macOS, or Windows.
Reading commands (`verify`, `log`, `status`, `doctor`) work anywhere Prikk builds; mutation does
not, by design — see [Platform Support](../reference/platform-support.md).

## `error: precondition not met: heads/<name> does not exist; run `prikk branch create heads/<name>` first`

`prikk branch switch` only moves to a branch that already exists. Create it from the branch you are
on with `prikk branch create heads/<name>`, then switch. The same command says `heads/<name> is
closed` for a closed branch: `prikk branch list` shows the open ones.

## `error: precondition not met: the active WAL holds unsealed work for heads/<name>; seal it …`

You committed on one branch and have not sealed, and are switching to another. Unsealed work belongs
to the branch it was committed on, so the switch refuses rather than leave it stranded. Seal it
(`prikk seal --allow-no-audit --ref heads/<name>`, exactly as the message prints), then switch. The
same message comes from `merge --into`, `branch create --from` and `archive import --adopt` when they
would move a branch that holds unsealed work: seal it, then run the command again.

## `error: precondition not met: the worktree is not clean against heads/<name>: <path> (modified), …`

The switch replaces the worktree's tracked files with the other branch's, so it refuses while any of
them differs from the branch you are on — it would otherwise overwrite or delete your edits. The
message lists each path and whether it is `modified`, `missing` or an `unsupported-path`;
`prikk worktree-status` shows the same list. Commit the changes, or restore the files, and switch
again. Untracked files do not count and are never touched. The same message (`… before creating
heads/<name>`, `… before merging <from> into <into>`, `… before sealing onto <ref>`, `… before adopting <ref>`) comes from the other commands that bring a worktree along
with a branch (`merge --into`, `branch create --from`, `sync seal` and `archive import --adopt` onto the
current branch); the way out is the same, and they refuse before they write anything.

**After a power loss on Windows, following a completed `branch switch` or `checkout` (RFC 168 residual (a)).** The rewritten files
can hold their old contents, deleted files can come back, and created files can be lost. Nothing in the repository is damaged.
`prikk worktree-status` lists what differs. The route, run in this order:

1. Move each file it lists as modified out of the worktree. The moved copy is yours: keep it or delete it.
2. For a file it lists as missing, do nothing: the checkout writes it.
3. For a file it lists as untracked that you deleted on the branch, a file that came back, delete it.
4. Run `prikk checkout --patch-materialize --ref <the current branch>`. It writes the branch's files into the paths that are now free.

`prikk worktree-status` should then list nothing for those paths.

## `error: precondition not met: refusing to switch to heads/<name>: <n> in the way of its files: …`

A file the other branch has would land on a path that already holds something that is not part of
the branch you are on — usually an untracked file with the same name. Nothing was written. Move the
listed paths aside and switch again.

**If a switch was interrupted** (a crash, a full disk), no file is torn and the current branch is still
the old one; `commit` then refuses with `worktree materialization was interrupted` (below). Run the same
`prikk branch switch heads/<name>` again: it recognises the half-switched files and completes.

## `error: precondition not met: <n> file(s) are in the way of heads/<name>'s files: <path> (…); move them aside before <action> (nothing was written)`

`merge --into`, `branch create --from`, `sync seal` or `archive import --adopt` was about to write the
current branch's new files into your worktree, and an untracked file already sits at one of those paths.
Nothing was written: `.prikk` and the worktree are as you left them. Move the listed paths aside, then
run the same command again. (`branch switch` says the same thing in its own words, above.)

If the file appears only after the branch has moved — another program created it in between — the
message reads `heads/<name> was published, but <n> file(s) are in the way of materializing it into the
worktree: …; move them aside, then run prikk branch switch heads/<name> to finish`. The branch is
published; the switch writes the files.

## `error: precondition not met: refusing to commit: heads/<name> is now at <state>, and this worktree was last written from <state>; …`

The branch moved without this worktree being told: another command moved it (an `archive import`
that reached it, a `sync seal`, a `merge`), or you committed with `--ref` onto a branch other than the
one you are on. Committing now would either record the files you did not receive as deleted, or
silently undo what moved the branch. The message lists every path and ends with the command that brings
the worktree up to date, `prikk branch switch <branch>`, with the branch named literally. Run it, then
commit again. When you committed with `--ref <x>` onto a branch you are not on, the command is the
switch to `<x>` itself. `prikk worktree-status` reports the same state as `behind heads/<name>` instead of
listing every new file as `missing`.

## `error: precondition not met: sync seal <ref>: claim <id> builds on block <id>, which <ref> does not reach …`

`sync seal` was asked to put a claim's patches on `<ref>`, but the claim was made on top of a block
that `<ref>`'s tip does not reach: `<ref>` has no history yet, or is behind that history, or is on
another line. Sealing it there would drop the history the claim was made on (a branch carried without
its earlier files), so nothing was written. The message names the next step. When a local branch
already holds that history, create the ref from it (`prikk branch create <ref> --from <branch>`; for a
ref that exists, create a new branch from it and seal onto that) and run the same `sync seal` again.
When no local branch holds it but the claims for that history are already accepted here (`prikk sync
pending` lists them), the message names those claims in lineage order, one
`prikk sync seal <ref> --claim <id>` per claim; run them in that order, then the refused command again
(a claims file that `sync accept --claims-out` wrote seals them in order too). When neither a branch
nor the claims are held, carry the branch it belongs to first (`sync have`, `sync build`,
`sync accept`, `sync seal` for that branch), then create `<ref>` from it. See [Sync](sync.md).

## `error: lock conflict: active lock already exists: <path> -- run prikk unlock to list held locks, …`

Another prikk process is writing this repository, or one that was writing it died and left its lock
file. The message is the same for the active-session lock, a ref lock and a container lock, and names
the file. `prikk unlock` lists every held lock with the recorded process id and start time and whether the process
that took it still appears to be running; `prikk doctor` reports each one as `PRIKK-DOCTOR-HELD-LOCK`. Wait for a
live process to finish. Clear a lock (`prikk unlock --lock <path>`) only after you have confirmed the
process that held it is gone: a lock that `appears running` is refused by `prikk unlock --lock <path>` unless you pass `--force`, which
clears it anyway and can corrupt what that process is writing; note that `does not appear to be running` and
`unknown` are not proof, and clearing a live holder's lock can corrupt the repository. The three words
are defined once, with what each platform records, in
[Concurrency and locking](../reference/concurrency-locking.md#what-the-three-liveness-words-mean).

## ``error: precondition not met: bundle import: this delivery classifies as `rollback` against remotes/<name>'s current pointer -- refusing …``

`bundle import` and `archive import` compare what they are given with the received pointer they would
replace, and refuse a delivery that is a `rollback` (older than what you hold), `diverged` (both sides
moved) or `does-not-connect` (no shared history), before writing anything. Nothing is wrong with your
repository; the delivery is not an extension of what you have. If you want it anyway — restoring an
older backup, say — pass `--allow-non-fast-forward`.

## `error: precondition not met: ref-state <id> carries a MAINTAINER signature by adopted key <key id> that does not verify: … a forgery, or another maintainer using the same key id; …`

`bundle import` or `archive import` found a Block or RefState signed under a key id you have adopted,
with a signature that does not verify against the key you adopted for that id. Either the delivery
is forged, or its maintainer is a different person who happens to use the same key id — most often the
legacy id `maintainer` that every installation made before 0.45.0 shares. Nothing was written. If it is
the second case, that maintainer signs under a distinct id (`prikk key generate --out <path>`, then
`PRIKK_MAINTAINER_SEED_FILE`; or `PRIKK_MAINTAINER_KEY_ID`) and you adopt that id with `prikk trust
maintainer add --key-id <that id> --public-key <its public key>`. `bundle preview` shows the same
signature as `invalid` before you import.

## `error: precondition not met: maintainer key id <id> is not currently adopted (it was removed), but it stays bound to the public key it was first adopted with, …`

`prikk trust maintainer add` refuses a different public key under a key id that was adopted once, whether
the id is adopted now (`is already adopted with a different public key`) or was removed with
`trust maintainer remove` (this message). A key id is bound to the first key it was adopted with, because
the id travels in every signature and a changed key under a familiar id is what trust on first use
exists to catch. Re-adding a removed id with its **original** key works. For a different key, its
maintainer signs under a distinct id and you adopt that id. With `--format json` the refusal is a
`refusal-v1` document whose `refusal` is `maintainer-key-id-adopted-with-different-key` or
`maintainer-key-id-bound-to-removed-key`.

## `error: <block id> is a block id; log does not show history ending at a block …`

`prikk log --ref` takes a ref name (`heads/…`, `tags/…`, `remotes/…`): it shows the chain of
publications that ref went through. A block has no such chain — it can have more than one parent — so
`log` refuses a block id instead of printing an empty history. To read what a block holds, use
`prikk show <block id>`, `prikk tree --ref <block id>` or `prikk cat --ref <block id>`. A block id that
names nothing here refuses with `block <id> is not in this repository`.

## `error: precondition not met: refusing to materialize: <n> path(s) in the way: …`

`checkout --patch-materialize`, `--patch-materialize-delete` or `--snapshot-materialize` found files
that would have to be overwritten: each is listed with why, such as `an existing file with different
content` or `not a regular file`. Nothing was written. This is the ordinary result of checking out
another branch's files over your own: move the listed paths aside, or commit them, and run the checkout
again. Earlier releases wrote every file before the first conflict, left the worktree part-written and
reported `integrity error: refusing to overwrite existing file with different content`.

## `error: precondition not met: refusing checkout deletion because <n> candidate(s) are unsafe: …`

`--patch-materialize-delete` would delete files that no longer match what history deleted; each is named
with the reason. Nothing was written. Earlier releases reported this as an `integrity error:` without the
paths.

## `error: precondition not met: <path> changed during the checkout, so the worktree is partly written; …`

A file the checkout had checked changed before it was written, usually because another program wrote it
at the same moment. The checkout stopped part-way and the worktree is marked as interrupted. Move that
file aside, then run one of the two routes below.

## `error: precondition not met: worktree materialization was interrupted, …`

A checkout or `branch switch` stopped part-way: a crash, or the change above. `prikk status` prints
`interrupted materialization: …` naming your current branch, `status --format json` carries
`interrupted_materialization` with the two routes, and `prikk doctor` warns
`PRIKK-DOCTOR-INTERRUPTED-MATERIALIZATION`. Until it is cleared, `commit` refuses, because it cannot tell
a file you deleted from one the stopped checkout never wrote. Move aside any file the stopped checkout
named, then run either
`prikk checkout --patch-materialize --ref heads/<current branch>` or
`prikk branch switch heads/<current branch>`; both write the branch's files again and clear it. A file
the stopped checkout had already written stays in the worktree as an untracked file, and `commit` authors
the whole worktree as it always does. Earlier releases reported this as an `integrity error:` whose
suggested route could refuse again.

## `error: precondition not met: no object <id> in the object store or the active WAL; …`

`prikk show` was given an id that names nothing here — usually a typo, or an id from another
repository. Sealed patch and block ids are listed by `prikk log`; queued (committed, not yet sealed)
patch ids by `prikk status --format json`, and `show` accepts both. Earlier releases reported this as
`error: integrity error: no object <id>`, which read as repository damage; nothing is damaged.

## `error: precondition not met: .prikk/current-branch names heads/<name>, which does not exist; …`

A command whose `--ref` you did not give defaults to the current branch, and `.prikk/current-branch`
names a branch that is not there. The same message says `which is closed` for a closed branch, and
`.prikk/current-branch is malformed (…)` when the file does not hold exactly one `heads/<name>` line.
Nothing is damaged, and both `prikk doctor` (the warning `PRIKK-DOCTOR-CURRENT-BRANCH`) and `prikk
verify` report it, as a warning that never changes either command's exit status. Either run `prikk
branch switch heads/<name>` to an existing, open branch — with an unusable pointer the switch writes
only what is absent and deletes nothing — or `prikk branch create` the branch the file names. Any
command still works with `--ref` given explicitly meanwhile.

## `error: invalid name: backslashes are not allowed in repository paths`

A file in the worktree has a name no repository path can hold — here a backslash; a name that is not
UTF-8 says `worktree path is not valid UTF-8: <name>` instead. `prikk commit` refuses the whole commit
rather than skip the file silently. `prikk worktree-status` lists it as an `unsupported-path` entry with
this same reason. Rename the file, or exclude it with a `.prikkignore` rule
([Ignoring Worktree Paths](ignore.md)).

## Something not listed here

Run [`prikk doctor`](tutorial.md#doctor) — it is the diagnostic-first command, and its recommendation
lines usually name the next step. If it does not, the message is real (Prikk does not invent
placeholder diagnostics) but this page has not caught up with it yet.
