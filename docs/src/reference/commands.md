# Command Surface

Every command prikk accepts, in one place. This page is an inventory, not a tutorial — each command's
own behaviour, refusals, and worked examples live in the [guide](../guide/tutorial.md), and the
[Git→prikk mapping](git-mapping.md) explains where the vocabulary differs from Git's.

Every command accepts `--help` for its own usage:

```text
prikk <command> --help
```

```text
prikk init [path]
prikk key generate [--out <path>]
prikk key public [--seed-file <path>] [--role author|maintainer]
prikk key status [path] [--role author|maintainer] [--format json]
prikk setup [repo-path] [--author-seed-out <path>] [--maintainer-seed-out <path>]
prikk trust maintainer add --key-id ID --public-key HEX [--format json]
prikk trust maintainer remove --key-id ID
prikk trust maintainer list [--format json]
prikk trust maintainer check --key-id ID [--format json]
prikk commit [--from-worktree] [--text-edits] [--ref heads/<branch>] -m <message>
prikk mv <old> <new>
prikk seal --allow-no-audit [--ref heads/<branch>]
prikk status [--format json]
prikk log [path] [--limit N] [--ref REF] [--format json]
prikk checkout --plan-only [path] [--ref <ref|block-id>]
prikk checkout --snapshot-plan [path] [--ref <ref|block-id>]
prikk checkout --snapshot-materialize [path] [--ref REF]
prikk checkout --patch-plan [path] [--ref <ref|block-id>] [--format json [--content-path <path>]...]
prikk checkout --patch-materialize [path] [--ref REF]
prikk checkout --patch-delete-plan [path] [--ref <ref|block-id>]
prikk checkout --patch-materialize-delete [path] [--ref REF]
prikk show <block-id|patch-id> [--format json]
prikk tree [path] [--ref <ref|block-id>] [--prefix <p>] [--format json]
prikk cat --path <p> [--ref <ref|block-id>] [--output <file> [--force]] [--max-bytes N] [--format json]
prikk extract (--ref <ref|block-id>|--claim <claim-id>) <dir> [--allow-full-replay] [--plan-only] [--max-bytes N] [--max-files N] [--format json]
prikk diff [path] [--from <ref|block-id>] [--to <ref|block-id>] [--path <p>]... [--format json]
prikk merge-evidence --baseline-block ID (--left-block ID|--left-ref REF) (--right-block ID|--right-ref REF) [path]
prikk merge-plan --baseline-block ID (--left-block ID|--left-ref REF) (--right-block ID|--right-ref REF) [path]
prikk merge --allow-no-audit --baseline-block ID --into REF --from REF [path]
prikk inverse-plan [path] [--ref REF]
prikk rollback-preview [path] [--ref REF]
prikk rollback-draft --append-inverse [path] [--ref REF] -m <message>
prikk rollback-draft-verify [path] [--ref REF]

prikk branch [list] [--all] [--format json]
prikk branch create heads/<name> [--from REF]
prikk branch close heads/<name>
prikk branch switch heads/<name>
prikk tag [list] [--format json]
prikk tag create tags/<name> --target <ref|block> [-m <message>]
prikk config get <key>
prikk config set <key> <value>
prikk config unset <key>
prikk config list
prikk bundle export --ref REF --output <file> [--force]
prikk bundle import --input <file> [--max-object-bytes N] [--allow-non-fast-forward] [--format json]
prikk bundle preview --input <file> [--ref REF] [--max-object-bytes N] [--format json]
prikk bundle verify --input <file> [--max-object-bytes N]
prikk archive export <file> [--format json]
prikk archive verify <file> [--format json]
prikk archive import <file> [--adopt] [--allow-non-fast-forward] [--format json]
prikk sync summary --output <file>
prikk sync compare --summary <file>
prikk sync have <ref> --output <file>
prikk sync build <ref> --have <file> --output <file> [--force]
prikk sync accept <file> [--max-object-bytes N] [--claims-out <file>] [--force]
prikk sync pending [--format json]
prikk sync seal <ref> --claim <id>
prikk sync seal <ref> --claims <file>
prikk sync tags
prikk sync adopt-tag <name>
prikk worktree-status [path] [--ref REF] [--format json]
prikk verify [path] [--stop-on-first-error] [--format json]
prikk doctor [path]
prikk doctor [path] --repair-wal-tail
prikk doctor [path] --repair-index
prikk doctor [path] --repair-pointer-index-tail
prikk doctor [path] --repair-tails [--plan-only]
prikk doctor [path] --repair-main-ref
prikk doctor [path] --rebuild-pointer-index [--plan-only]
prikk doctor [path] --rebuild-commit-index [--plan-only]
prikk doctor [path] --discard-damaged-commits [--plan-only]
prikk doctor [path] --restore-queue-target --ref <ref> [--not-current-branch] [--plan-only]
prikk doctor [path] --recovery-list
prikk doctor [path] --recovery-restore <run id> [--plan-only]
prikk doctor [path] --recovery-clear [--plan-only]
prikk doctor [path] --cancel-import <id> [--plan-only]
prikk ref complete <ref> [--plan-only]
prikk format upgrade [path]
prikk unlock
prikk unlock --lock <path> [--yes|--force]
prikk compact --pointer-index|--received-index|--trust-policy|--all [--plan-only]
prikk compact --received-index|--trust-policy --keep-slot a|b [--plan-only]
prikk received remove <remotes/…> [--plan-only]
prikk --version
```

**`doctor --rebuild-commit-index` rebuilds a cache.** `.prikk/cache/commit-index.v1` only saves `commit` a
read; it holds no history, so the verb writes no recovery-log entry. It reads every file `commit` would
scan once (the current branch's baseline decides each file's kind) and writes a correct index, under the
active lock `commit` takes: a `commit` running meanwhile fails with `lock conflict` instead of racing it.
A file written too recently to trust (see below), changed while being read, or holding content `commit`
refuses is read but left out. `--plan-only` counts the files, takes no lock and writes nothing. It runs
alone (only `--plan-only` goes with it). `verify` and `doctor` name it when they find a commit-index
divergence.

**`commit` does not record an entry for a file written inside the file system's current timestamp tick.**
An entry is left out when the file's mtime is not older than the file system's own clock at the moment
`commit` took its lock (the creation stamp of the lock file, so the comparison holds on a coarse-clock
file system too). The next `commit` reads that file once; nothing else changes. This closes the
cache-validity specification's first residual-risk class (a rewrite in the same tick as the recorded
mtime); an mtime put back by a tool stays `verify`'s to find.

**What `--from` and `--target` accept.** `branch create --from REF` takes any published ref: a
branch ref names its Block directly, and a tag ref is dereferenced through its tag object, so
`--from tags/v1` publishes the new branch at the Block that tag names. `tag create --target` takes a
block id or a **branch** ref, and **refuses a tag ref** — a tag of a tag is outside the model (ref →
tag object → block, one hop), and resolving it silently would make `--target tags/v1` and `--target`
that tag's own block indistinguishable in history. The refusal names both accepted forms.

**Key ids.**
- `setup` (when it creates a seed) and `key generate --out` write the new key's id,
  `ed25519-<first 16 hex of the public key>`, beside the seed, as `<role>.key-id` in the key directory or
  `<seed path>.key-id`.
- Every signer, `setup` and `key status` resolve the id the same way: `PRIKK_<ROLE>_KEY_ID`, else that
  file, else the legacy role word.
- `setup` adopts the maintainer key under the resolved id and prints it.
- `key status` reports `key_id_source` as `environment`, `key-file` or `default`, and a key-id file that
  does not hold its seed's id as `reason: key-id-file-mismatch`, which refuses signing.

**`format upgrade` is explicit, verified and one-way.** A repository created by 0.44.0 or earlier is
format 6; new repositories are format 7, which is format 6 plus "an object id may hold several records,
the last authoritative". `prikk format upgrade` takes the writer lock, runs the same verification as
`prikk verify` and refuses — changing nothing — unless it would exit 0, then overwrites the one byte
of the `.prikk/FORMAT` marker in place and flushes it (no rename; RFC 168 §3.3, `crates/prikk-store/src/format_upgrade.rs:81-88`). Nothing stored is rewritten. It is idempotent on a format-7
repository, never runs by itself, and has no inverse: prikk 0.44.0 and earlier refuse a format-7
repository at open. See [Release Compatibility](release-compatibility.md).

**Refusals about refs and repositories.**
- **A ref that does not exist.** A command that reads an existing ref's state refuses an absent name
  before anything else with `precondition not met: ref <name> does not exist in this repository` (exit
  `1`), adding a sentence when `remotes/<name>` exists. That covers `log` and `worktree-status` given
  `--ref`, every `checkout` mode, `inverse-plan`, `rollback-preview`, `rollback-draft`,
  `rollback-draft-verify`, `merge-evidence`, `merge-plan`, `merge --into`/`--from`, `bundle export`,
  `branch create --from`, `tag create --target`, `branch close` and `branch switch`, which keeps its own
  route sentence.
- **Not refused:** `sync have`, `sync build` and `bundle preview` answer an absent ref as a state ("none
  of it"), and `commit --ref`, `seal --ref` and `branch create` create the ref. In a fresh repository,
  `log` and `worktree-status` without `--ref` still report the unpublished current branch.
- **A received ref given to a command that cannot use one** (`checkout`, `inverse-plan`,
  `rollback-preview`, `bundle export`, `branch create --from`, `tag create --target`) refuses with
  `precondition not met: <name> is a received ref`, naming the commands that read received refs (`log`,
  `merge-evidence`, `merge-plan`, `bundle preview`) and the one that takes it into a local branch
  (`merge --from`).
- **A path with no repository** refuses as a precondition naming that path ("no prikk repository at
  <path>"), decided when the repository is opened, in every command that opens one.

**The worktree's recorded baseline.** `setup`/`init`, `branch switch`, both
`checkout --patch-materialize`/`--snapshot-materialize` writers, `commit` and `seal` each record
what this worktree was last made to match: a ref's sealed tip block (or none, for an unborn ref)
plus a content identity over whichever of the active WAL's queued patches are folded on top, in
their own order — not a bare count, so a `doctor` repair that removes one specific queued patch is
told apart from an identical commit retried after a crash. `seal` advances the record only when the
worktree already matched the fold it sealed; a fold the worktree was never written to match (the one
`rollback-draft --append-inverse` queues) leaves the record where it was. This is worktree-local
operational state, like the unclean-shutdown marker beside it — never part of any versioned object,
and `verify`/`doctor` do not look at it.
- **`commit` (and `commit --ref <x>`) refuses to author over a path that is stale against this
  worktree's own record — content, mode, kind, presence, or the node it belongs to — unless the
  worktree already holds the live state for it, and this holds in exactly the same way whichever
  ref the record itself names.** The refusal comes before anything is authored and names every such
  path: ``precondition not met: refusing to commit: <ref> is now at <state>, and this worktree was
  last written from <state>; …; run `prikk branch switch <ref>` to bring the worktree up to date,
  then commit again``. Every command the refusal names, run as written, is followed by the same
  `commit` going through (or by a refusal of its own that says why). When the commit is onto the
  current branch, the way out is `prikk branch switch <the record's own ref>`, with the ref the
  record actually names substituted literally (the record is an accurate description of what the
  worktree physically holds), and also the self-switch, `prikk branch switch <the current branch>`,
  when the ref being committed onto is itself published. When the commit is onto any other ref
  (`commit --ref <x>`, `<x>` not the current branch), the only command named is `prikk branch switch
  <x>`, with `<x>` substituted literally: switching to the record's ref or to the current branch
  would change nothing there, and the same commit would refuse again. A refused `commit` leaves
  `.prikk` unchanged except for its cache, as `worktree-status` itself does. A path missing from the worktree that the
  record never explains reads "not in this worktree, and never were" (the deletion-shaped case: a
  command moved the ref without writing the worktree, or `commit --ref <other>` ran from a worktree
  recorded against a different branch). A path the worktree still physically holds, but whose
  content, mode, or node disagrees between the record and the ref's current state, reads as the
  worktree holding content that does not match the ref's current state — authoring it now would
  either revert whatever moved the ref underneath this worktree, or record a conflict (the worktree
  holding a third state) as an ordinary edit. Either way the commit refuses rather than guess.
  Staleness is judged per path, in states, never by comparing ref names: committing onto a ref other
  than the one the record names is ordinary and unrefused on every path where the record and that
  ref's live state agree (shared, undiverged history), and a path where they disagree refuses
  exactly the same way whichever ref the record names, including an edit that would otherwise
  silently revert a sibling branch's own diverged change. A path that is not stale, or one the
  worktree already matches the live state for, authors as usual, including a genuine deletion, edit,
  mode change, or rename of content this worktree really did hold.
- **The commands that move the current branch bring a clean worktree along, or refuse before they
  write anything:** `merge --into <the current branch>`, `branch create --from <ref>` when the new
  branch is the unborn current branch, `sync seal <ref> --claim`/`--claims <file>` onto the current
  branch, and `archive import --adopt` of a ref destined for the current branch. Each follows one
  rule:
  1. The worktree must be clean against its own record. A dirty worktree refuses the command, naming
     the paths `worktree-status` would: ``precondition not met: the worktree is not clean against
     <ref>: <path> (modified); commit it, or restore those files, before <action>
     (`prikk worktree-status` lists them)``.
  2. A file already on disk at a path the new tip's files occupy, untracked so the clean check does
     not refuse it, is in the way: `precondition not met: <n> file(s) are in the way of <ref>'s
     files: <path> (<why>); move them aside before <action> (nothing was written)`. This check is
     part of the command's own read-only refusals, so a refused command writes no object and leaves
     `.prikk` unchanged except for its own cache (`verify` reports no unreached object afterward).
     Moving the path aside and running the command again is all a caller needs to do.
  3. Past those refusals the command marks the worktree dirty, publishes exactly as it always did,
     and writes the new tip's files into the worktree through the same verified path `branch switch`
     uses, then records the new baseline. It prints `materialized into the worktree: files written:
     <n>, unchanged: <n>, deleted: <n>`, and `--format json` on `archive import` carries the same
     counts as `materialized_into_worktree`, `materialized_written_files`,
     `materialized_unchanged_files` and `materialized_deleted_files`. The next ordinary `commit`
     finds nothing stale. If a file appears in the way after the publish (created by a process
     outside prikk), the error says the ref was published, names the path, and says to move it
     aside and run `prikk branch switch <the current branch>`, which finishes the write.

  Each command then adds only what is specific to it:
  - **`merge --into`** derives the merge Block's own state before it writes anything, so the
    in-the-way check judges the files the merge will actually commit, including a file whose latest
    content exists only as a text edit.
  - **`branch create --from`** checks the unborn current branch's own (empty) baseline and the
    `--from` target's files.
  - **`sync seal`** treats a single `--claim` as a batch of one. The clean check and the in-the-way
    check run once, before the first claim seals, against the state after the claims that will
    actually seal (folded claim by claim in sealing order, stopping at the first claim that does not
    fold). The worktree is materialized once, after the last claim, from the ref's real final tip.
    Sealing in order and stopping at the first failure, with no rollback, is unchanged: when a later
    claim fails after an earlier one has published, the earlier seal stays, the tip it produced is
    still written into the worktree, and then the batch's error is reported; if that write itself
    fails, the error also names `branch switch <the current branch>`. A batch that publishes nothing
    (every claim refused outright or already sealed) writes nothing into the worktree and prints no
    `materialized` line. Sealing onto a ref that is not the current branch, or a `--claims <file>`
    that names no claims, takes none of these steps.
  - **`archive import --adopt`** runs both checks before the import journal exists, so nothing from
    the archive has been admitted when either refuses. The in-the-way check reads the archive's own
    Patch and Blob sections in place, and adds about 9% to the import's total time at
    10,000–20,000 objects (release build, in-memory filesystem, `archive import --adopt` of one
    genesis generation). The
    clean check runs again, together with a second in-the-way check, immediately before the
    publish that moves the current branch — on a fresh import and a resumed one alike — as the
    backstop for a file created by a process outside prikk in between; the backstop's message says
    the archive's objects are already admitted and the journal written, and that running the same
    import again resumes it. Resuming an import that crashed between a landing's publish and its
    write into the worktree reports that landing as `adopted`, noting it was completed by an
    earlier, interrupted run (`completed_by_interrupted_run: true` in `--format json`), writes no
    `remotes/` pointer for it, and finishes the write into the worktree itself, so no `branch
    switch` is left to run by hand. That report is made only for a genuine resume of this import's
    own journal, and only when the ref was absent when the import's plan was first computed: a ref
    that already holds the same tip because some other, earlier import adopted it reports the
    ordinary `remotes/` fall-through, exactly as an uninterrupted run would. A journal written by
    an older binary, from an import still in progress across the upgrade, carries no plan-time
    record and resumes with that ordinary fall-through. A landing not destined for the current
    branch, or one `--adopt` cannot land as a real local ref (it falls through to `remotes/`), takes
    none of these steps.
  - **`rollback-draft --append-inverse`** moves only the active WAL's queued fold, never a sealed
    block or the worktree, and says so: ``note: the worktree is unchanged by this draft; run `prikk
    seal`, then `prikk branch switch <ref>`, to apply the inverse``.
- **`branch switch <the current branch>` is a no-op only when the record is current.** When the
  record shows the worktree behind that branch's live *sealed* tip, naming the current branch
  materializes the branch's current files and clears the staleness, the same write-then-publish
  choreography switching to a different branch uses. Both directions of a switch (leaving a
  branch, and this same-branch resync) judge "clean", and which files the old baseline held, against
  the worktree's recorded state whenever a record exists and disagrees with the branch's live one —
  **whichever ref the record itself names**, not only when it names the branch being left or
  switched to — and fall back to the live tip itself only when no record exists, or the record
  already agrees with it. That is what lets a switch away from a stale branch, and the same-branch
  resync, both succeed after a `commit --ref <other>` left the record naming a different ref than
  the current-branch pointer. Switching to a *different* branch materializes only that branch's
  sealed tip, never its queued fold.
  - **The self-switch also resyncs when the current branch has never been published** (no `RefState`
    exists for it yet) and the record reads behind it — the `worktree-patch.md` walkthrough's shape:
    `commit --ref heads/topic` on a genuinely unborn ref, then `seal`. It materializes the branch's
    own empty (never-published) state, judges "clean" against the record the same way every other
    behind-reading switch does, and records that — for this one shape only; every other target
    refuses without a published `RefState`, and the open-or-closed check is unaffected.
  - **A moved, queued, unsealed fold** — as `rollback-draft --append-inverse` moves it without
    touching the sealed branch or the worktree — is detected the same way, but a bare same-branch
    resync never writes it into the worktree directly: that would bypass the verified replay every
    other switch uses. The self-switch over a non-empty, behind-reading queue refuses instead,
    naming `seal` then `branch switch <the current branch>` again; because `seal` leaves the record
    where it was for a fold the worktree never matched, that second switch resyncs through the
    ordinary verified path.
- **`worktree-status` reports `behind <ref>`** instead of listing every file a moved ref added as
  `missing`, specifically when the record shows this worktree behind the ref it is reporting on —
  its sealed block, its queued fold, or both — (not when a file was genuinely deleted, which still
  reports as `missing`). The text form prints ``worktree: behind <ref> (<n> file(s) it added since
  this worktree was last written from it are not here yet; not a deletion); run `prikk branch switch
  <the current branch>` … to catch up``. `--format json` carries this as an additive `"behind_ref"`
  field beside `"queued_elsewhere"`, `null` when the worktree is current or no record exists; its
  `changes` list is unchanged.
- **`merge --into`, `archive import --adopt` and `branch create --from` refuse to move a ref the
  active WAL already holds queued, unsealed work for.** Each checks before any write and refuses,
  ``precondition not met: the active WAL holds unsealed work for <ref>; seal it (`prikk seal
  --allow-no-audit --ref <ref>`) before <action>``; a queue that targets a *different* ref, or no
  queue at all, is unaffected. `sync seal --claim` and `rollback-draft --append-inverse` refuse on
  *any* non-empty active WAL regardless of which ref owns it, a strictly stronger condition that
  already covers this case for both.

**`sync seal` judges a claim against the target ref's own history.** A claim's patches count as
already sealed (`claim <id>: <ref> was already fully sealed -- no-op`) only when a block reachable
from `<ref>`'s current tip carries them; a block some other ref sealed does not count, so the line is
never printed for a ref that does not hold the patches. And a claim whose `parent_block_ids` name a
block that `<ref>`'s tip does not reach is refused before anything is written, whole batch included:
sealing it would put the patch on whatever the tip happens to be and drop the history the claim was
made on. The refusal is `error: precondition not met: sync seal <ref>: claim <id> builds on block
<id>, which <ref> does not reach (<ref> has no history yet | is at block <id>) …`, exit `1`, with no
`refusal-v1` document (`sync seal` has no `--format json`), `.prikk` unchanged, and a way out: when a
local branch reaches that block, `prikk branch create <ref> --from <branch>` (for an unborn `<ref>`)
or `prikk branch create <new-branch> --from <branch>` and seal onto it (for a `<ref>` that exists but
is behind that history or on another line); when no local branch does but the claims for that history are
already accepted here, those claims in lineage order, one `prikk sync seal <ref> --claim <id>` per
claim, then the refused command again; when neither is held, the commands that carry the branch it
belongs to first (`sync have`, `sync build`, `sync accept`, `sync seal`), then `prikk branch create
<ref> --from <that-branch>`. "Reaches" is judged by patch, because block ids differ between
repositories by design: a parent counts as reached when its id is in the tip's history, or when every
patch of its own ancestry that this repository can name (from the block when held, else from the
claims held for it) is already in the tip's history. A parent that is neither a held block nor the
subject of a held claim cannot be judged and is not reached. A batch that carries a branch's whole
chain, in a file, onto an unborn `<ref>` seals it claim by claim and passes, since each claim's
parent is the one before it.

**`bundle import` and `bundle preview` classify a delivery against the pointer it would replace,
in six classes:** `no-local-history` (nothing to compare against), `already-at-tip` (identical to
what is already there), `rollback` (the delivery is older than what is already there),
`fast-forward` (the delivery extends what is already there), `diverged` (both sides moved since a
shared point), and `does-not-connect` (no shared history at all). `bundle preview --format json`
reports this as an additive `"class"` field — its existing `"connectivity"` field keeps its
original five values unchanged (`already-at-tip` and `rollback` both still read
`"already-included"` there), so nothing a script already reads changes. Both commands also report
`"ref_state_chain"`: `"extends"`, `"older"`, or `"unrelated"` — how the delivery's own publication
history relates to the pointer it would replace, which can disagree with the class above (a
`fast-forward` by content can still be `"older"` in its own publication history).
- **`bundle import` refuses a `rollback`, a `diverged` delivery, or one that `does-not-connect`,
  whenever a received pointer already exists for that name** — naming the class, exit `1` — unless
  `--allow-non-fast-forward` is given. A delivery with no existing pointer to replace, or one that
  is `already-at-tip` or a genuine `fast-forward`, is never refused on this ground.
- **An `already-at-tip` delivery whose own publication history reads `"older"` writes every object
  it carries, but leaves the received pointer itself unchanged, and exits `0`.** The content is
  identical either way, and moving the pointer backward would only make its own publication
  history read incorrectly later; the text form says plainly that the pointer was left as it was.

**`bundle import --format json`: `bundle-import-report-v1`.** On success it carries the same
fields the text form already prints — `ref_name`, `ref_state_id`, `object_count`,
`written_object_count`, `merged_object_count`, `dropped_signatures` (each
`{"object_type", "object_id", "key_id", "reason"}`, the same four-field shape `archive import`'s
own `prepo-import-report-v1` already uses for this type), `recorded_author_key_count` — plus
`class` and `ref_state_chain` (the same names and values as `bundle-preview-v1`'s own fields,
above) and `pointer_updated` (`false` only for the `already-at-tip`-with-`"older"` case just
described, which writes every object but leaves the pointer itself untouched). The text form
itself is unchanged by this.
- **On a refusal, `bundle import --format json` prints a `refusal-v1` document** (the same shape
  `extract`'s own `refusal-v1` uses, see below) with `"command": "bundle-import"`, to stdout, on
  stderr and the exit code otherwise unchanged — for exactly the three refusals above
  (`"refusal"`: `"rollback"`, `"diverged"`, or `"does-not-connect"`), and for the two refusals
  described further below (`"forged-publication-signature"`, `"outside-version-rule"`), never for
  `bundle import`'s other refusals (a damaged bundle, a digest mismatch, an author-key conflict, and
  so on), which print no document. `"reason"` is always `null`.

**`archive import` applies the identical classification and refusal to its own landings, plus
`--adopt`.**
- **A received landing** (the default, and `--adopt`'s own fallback for a ref it did not adopt)
  classifies against the received pointer of the same name and refuses a `rollback`, a `diverged`
  landing, or one that `does-not-connect`, naming the class, exit `1`, unless
  `--allow-non-fast-forward` is given — the same three names and the same flag `bundle import`
  uses. A landing with no existing pointer to replace, or one that is `already-at-tip` or a genuine
  `fast-forward`, is never refused on this ground, and an `already-at-tip` landing whose own
  publication history reads `"older"` leaves the received pointer itself unchanged, the same as
  `bundle import`.
  Every planned landing is classified against the local repository before anything is written at
  all, not only after every carried object has already landed, so a refusal leaves the repository
  exactly as it was found: no object written, no unfinished import record behind. Resuming an
  interrupted import (after a real crash, say) still works the same way, and still catches the
  rarer case of the pointer moving again in the gap before the resume.
- **`--adopt` also checks the received pointer already held at the name a ref would otherwise land
  under** (`remotes/<ref>`), before adopting it as a real local branch. The same three classes
  refuse the same way, under the same flag. When the flag lets the adoption through anyway, the
  landing's own `"remote_reason"` names the class, even though adoption itself succeeded — the one
  case this field carries something other than why a ref fell through to `remotes/` instead of
  being adopted. When a local branch of that name already exists, `--adopt` lands the delivery as
  `remotes/<name>` instead, classified against the received pointer only (often `no-local-history`,
  and the first such landing reads that next to "a ref by this name already exists locally"). The
  local branch is neither compared nor moved, and the class id is unchanged.
- **`--format json` reports the same classification `bundle import --format json` does.** Each
  entry in its own `"landings"` array carries `"class"` and `"ref_state_chain"` (the same names and
  values described above, for every landing, not only a refused one), and a refusal prints the
  same `refusal-v1` document described above, with `"command": "archive-import"`.

**`archive import` and `archive verify` refuse a `Patch` whose own operations do not decode,
before anything is written** — the identical check `bundle import`'s own admission runs. All three
commands apply the same check, so a change `bundle import` would refuse is refused by
`archive import`/`archive verify` too, never landed or reported as internally consistent.

**`archive verify` reports each carried maintainer key's real status in the receiving repository.**
Both the text form and `--format json`'s `"status"` field carry one of four fixed strings, computed against the repository `archive verify` runs from, never against the
archive's own carried trust policy: `"adopted"` (the same key id and the same public key are
already adopted here); `"carried, not adopted"` (this key id is not adopted here at all);
`"carried, conflicts with an adopted key"` (this key id is adopted here, but under a *different*
public key); or `"carried, not checked against a repository"`, for either of two cases: `archive
verify` is run outside any repository at all (silent, with nothing to check against), or a
`.prikk` is present but could not be read — a corrupted `FORMAT` marker, or a corrupted
trust-policy container — which also prints exactly one line to stderr naming that a repository is
here but unreadable. This is reporting only — the archive's own internal-consistency check, and its exit code, are
unaffected either way, exactly as for the existing carried-key listing this refines; trust remains
the receiver's own adopted set, never the archive's.

**`bundle import` and `archive import` refuse a forged MAINTAINER signature on any Block or RefState
they would admit, before anything is written.** A signature that names a key id this repository has
adopted but does not verify against that key (a forgery, or another maintainer who happens to use the
same key id) refuses the import — checked on every Block and RefState the delivery carries, not
only its tip, each once — with the same check `verify` and `seal` apply to publication objects
(`PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE`). The message names both possible causes and a route
for the honest one: the sending maintainer signs under a distinct key id (`prikk key generate --out
<path>` makes one, pointed at by `PRIKK_MAINTAINER_SEED_FILE`; or `PRIKK_MAINTAINER_KEY_ID`), and the
receiving repository adopts that id with `prikk trust maintainer add --key-id <that id> --public-key
<its public key>`. Two installations made before 0.45.0 share the default id `maintainer`, so a
delivery between them refuses here instead of importing and failing a later `verify`. A signature
by a key this repository has never adopted is a different condition
(`PRIKK-TRUST-PUBLICATION-UNTRUSTED`) and is not refused: that history still imports, untrusted, and
the way to gain trust in it is `trust maintainer add` followed by an ordinary `verify`. Under
`--format json`, the refusal prints the `refusal-v1` document described above, with `"refusal":
"forged-publication-signature"`.

**`bundle preview` reports each sealer's own publication-trust outcome, additively.** Alongside
the existing `"sealed_by"` field (a plain list of signing key ids, reported but never trusted,
unchanged by this), an additive `"sealed_by_signatures"` field lists the same signatures with an
`"outcome"` for each: `"sound"` (the key is adopted and the signature verifies), `"unverifiable"`
(the key is not adopted by this repository at all), or `"invalid"` (the key is adopted, but the
signature does not verify — the same condition `bundle import` refuses on). This is reporting
only, exactly like `"sealed_by"` itself — `bundle import` is still the only thing that acts on it.
The outcomes are for the signatures on the delivered tip, so a `sound` here does not by itself mean
the import will admit the delivery; the next paragraph is what answers that.

**`bundle preview` lists every refusal `bundle import` would raise, in the import's own order.** An
additive `"import_refusals"` array (the schema stays `bundle-preview-v1`) is `[]` when the import,
run without flags, would admit the delivery. Otherwise each entry is `{"id", "message",
"object_type", "object_id"}`: first the delivery-class refusal (`id` is `rollback`, `diverged` or
`does-not-connect`; `object_type` and `object_id` are `null`, because the sentence names the received
pointer), then `forged-publication-signature` with the first Block or RefState whose MAINTAINER
signature by an adopted key does not verify, and its sentence. Each entry comes from the import's own
function, not a restatement, and `message` is the sentence the import prints. It is a list because one
refusal can hide another: **a class refusal is lifted by `--allow-non-fast-forward`, and a signature
refusal by nothing**, so a user who passes the flag meets the second entry. The text form prints one
`bundle import of this delivery would refuse: <id>` line per entry, the sentence indented under it, and
nothing when the list is empty. The signature entry costs what the import's own check costs: one Ed25519
verification per signature of each unique Block and RefState the delivery carries (about 26 µs each,
measured at 0.56.0). It covers the publication-signature check only; see the limit below.

**One refusal the list does not name.** The import first admits the signatures of objects it already
holds (RFC 156): a delivery that re-sends a held object with an extra MAINTAINER signature by an adopted
key that does not verify is refused there, with `invalid signature: … refusing the whole import, nothing
was written`, before the class check. `bundle preview` does not run that step.

**`bundle preview --ref remotes/<name>` resolves what was actually received at that name.** A
`remotes/…` name answers against the received pointer held under that exact name; if none is held
there, the command refuses by name rather than reporting a misleading "no local history" answer. A
`heads/…` or `tags/…` name is unaffected. `bundle preview` also always reports, in two additional
fields (`"import_class"` and `"import_ref_state_chain"`), the classification `bundle import` of the
identical delivery would actually make — against the received pointer this repository would
actually write to, which is not always the same name a `--ref` comparison asked to inspect. These
are additive and distinct from `"class"`/`"ref_state_chain"` above, which answer only the `--ref`
comparison.

**A path this version of `prikk` does not accept is not damage.** Every reader (`verify`,
`doctor`, `show`, `tree`, `extract`, and `commit` when it replays existing history to derive
state) and every importer's admission check (`bundle import`/`verify`/`preview`,
`archive import`/`verify`) classify a path that fails this version's own path rule (today:
non-ASCII) distinctly from genuine corruption: the message names the path and says that it is
outside what this version of `prikk` accepts, and that a newer version of `prikk` may have
written it — never "malformed", never "does not decode", and never a roadmap statement about
when the rule might change. `doctor`'s own recommendation for this case says no repair is
needed and to read the repository with a newer version of `prikk`, rather than telling the
operator to preserve the repository and inspect the failing object or block. A worktree file
with such a name, reached while authoring a commit (including from `diff`'s and `status`'s own
worktree scans), is told something different instead: that this version of `prikk` does not
accept non-ASCII repository paths, naming the file. Every one of these commands refuses; the
classification and the wording say why. `bundle import --format json` and
`archive import --format json` print a `refusal-v1` document for this cause (`"refusal":
"outside-version-rule"`, the same id `extract --format json` gives).

**A point: a ref or a bare block id.** `checkout`'s read-only modes (`--plan-only`, `--snapshot-plan`,
`--patch-plan` with or without `--content-path`, `--patch-delete-plan`) take `--ref` as a ref name or a
bare block id, 64 lowercase hex characters, resolved by one resolver.
- A block the repository holds resolves whether or not any ref reaches it: an older block on a branch,
  or a block only received history holds. Reading it adopts nothing.
- `block <id> is not in this repository` and `object <id> is a patch, not a block` (naming the type it
  is) refuse as preconditions, exit `1`. A `--ref` value that is neither a ref name (`heads/…`,
  `tags/…`, `remotes/…`) nor 64 lowercase hex is a usage error, exit `2`.
- The modes that write the worktree (`--snapshot-materialize`, `--patch-materialize`,
  `--patch-materialize-delete`) refuse a block id: writing the worktree needs a branch, because the
  next `commit` authors against one.
- The prose header reads `block: <id>` for a block id and `ref: <name>` otherwise; `--plan-only` prints
  `ref-state: <none>` for a block id. `patch-plan-content-v1`'s `ref` holds the value as given, now
  possibly a block id; its schema version is unchanged.

**`log --ref` and a bare block id.** `log --ref` is resolved by the same one resolver described
above. A block id naming nothing in this repository refuses as a precondition (`precondition not
met: block <id> is not in this repository`, exit `1`); a value that is neither a ref name nor 64
lowercase hex refuses too (`invalid name: <value> is not a point this command reads: expected a ref
name (heads/…, tags/… or remotes/…) or a block id (64 lowercase hex characters)`, exit `1`); and a
block id that does resolve refuses, because `log` shows the publication chain of a ref, not a
block's own ancestry: ``<id> is a block id; `log` does not show history ending at a block -- a block
can have more than one parent, so there is no single next entry to walk; expected a ref name
(heads/…, tags/… or remotes/…)`` (exit `1`). `log --ref` never answers a block id with an empty
history.

**`show`'s own `state` field, and resolving a pending text edit's path.** Every patch
`show --format json` reports (`show-report-v1`, additive) carries
`"state"`: `"queued"` (found only in this repository's own active WAL), `"sealed"` (named by a
held Block's own `patch_ids` — the same membership `sync pending` subtracts, independent of
whether any ref, local or received, still reaches that Block), `"pending"` (not sealed, but named
by a held recognition claim — the same membership `sync pending` itself reports), or `"unclaimed"`
(held, but sealed by no Block and named by no held claim — a hand-made patch accepted without
ever naming a claim, or a crashed or interrupted import's leftover, reads this way). `state` is the
same whichever id names the patch, a block's own bare id included: a patch a rollback or a
`received remove` left unreached by any tip still reads `"sealed"`, because its own Block is still
held — a bare-id read never needed a reachability check at all, and neither does this. `"sealed"`
and `"pending"`/`"unclaimed"` cannot both hold for the same patch; `"queued"` and the other three
cannot either, under ordinary operation, since a queued patch has no written object yet at all.
`"queued"` and `"sealed"` can briefly overlap across the window between sealing a block and
clearing the queue afterward — `show` resolves this by looking for a real object before ever
falling back to the queue, so `"sealed"` wins.

A pending patch's own node-addressed operations (`edit-text`, `replace-binary`, `change-perm`)
resolve a real path when a recognition claim naming it leads back, through as many further held
claims as it takes, to a real parent block this
repository already holds, sealed or received — folding every link's own patches in order onto that
real parent's state, the same way a sealed block's own patches fold onto its parent's. A claim's
own declared parent need not itself be sealed or received: it can instead be the prospective block
*another* held, unsealed claim recognizes, and that claim's own parent carries on from there, and
so on, the identical chain walk `extract --claim` uses. When the chain resolves the path, that
patch's own `"resolved_through_claim_id"` names the claim given on the command line (the chain's
own foot), never an earlier link in its own chain; a tool can look up that claim's own signature
outcome in `sync pending`'s own `sync-pending-v1` listing. A claim whose own MAINTAINER signature
is invalid is skipped outright, never used to choose a path — checked at every link the chain
walks through, not only the one named directly; a link whose own patches do not actually fold
against its own declared parent is skipped the same way, its own fold failure named as the reason
— in neither case does `show` itself refuse. **The path stays unresolved** when every claim naming
the patch fails: some link's declared parent is itself only
pending, part of a chain of claims that is not yet realized; some link names more than one parent,
with no mainline to pick; the chain needs to resolve a block that two or more held claims
recognize, with none or more than one of them signed by an adopted key (a shared-block collision);
or the chain loops back on itself (a cycle). `"unresolved_node_id"` carries a `"reason"` alongside
it in every such case, naming which applies, rather than guessing a path — and when more than one
claim fails for a different reason, the most specific one is reported, in this order: an invalid
signature, a fold failure, a shared-block collision, a cycle, a chain, then a merge (the first two
are about the failing claim's or link's own soundness; the collision and the cycle are specific,
nameable facts about the store's state; the last two name only where the walk stopped, with
nothing more specific to go on). A bare sealed patch id shown directly stays unresolved too, also
naming why: `show` does not search for the block that sealed a patch shown by its own id; `prikk
log` or `prikk tree` finds it. **An `"unclaimed"` patch is classified before any of this is even
attempted**: since no held claim names it, there is nothing to resolve a path
through at all, and its own operations are reported unresolved with that reason directly.

**`sync pending` and its own `--format json`: `sync-pending-v1`.** `sync pending` lists patches
this repository holds that are named by a held recognition claim and named by no held Block:
patch objects present, minus those any held
Block's own `patch_ids` names, intersected with those a held claim's own `patch_ids` names. A
patch sealed by a held Block is excluded whether or not any ref — local or received — still
reaches that Block: a rolled-back block, or one a `received remove` orphaned, still excludes its
own patches, since the Block object itself, not any ref pointing at it, is what this listing
subtracts. This is a listing-only membership: `sync seal` is unchanged, still sealing a claim's
patches into this repository's own history regardless of what this listing shows.
`--format json`'s one document, `sync-pending-v1`, adds the recognition claim(s) naming each pending
patch: `patches[]`, each `{"patch_id", "claims"}`; `claims[]`, each `{"claim_id", "target_block_id",
"parent_block_ids", "signature"}`, where `signature` is `{"outcome": "sound"|"unverifiable"|"invalid", "key_id"}` from
the claim's own MAINTAINER signature. **`"invalid"` is the one value a tool consuming this listing
must treat as an alarm:** a detected forgery under a key this repository has adopted, a
MAINTAINER signature whose algorithm is not Ed25519, or no MAINTAINER signature at all — carried
alongside `"reason"` (the refusal's own text) and, for the first two cases, `"key_id"`; the last
case's `"key_id"` is `null`, since there is no signature to name one from. This is reported for
that one claim only, beside every other pending patch — it never refuses the whole listing, unlike
`sync accept`/`sync seal`, which still refuse outright on the identical signature check.
`target_block_id` is the block this claim recognizes — the same field and meaning as
`extract-report-v1`'s own `target_block_id` (see `extract`'s own `--format json` below).
`parent_block_ids` is the block(s) this claim declares as `target_block_id`'s own parents — the
same meaning as `extract-report-v1`'s own `claim_parent_block_id`, but as a **list**: `[]` for a
root claim, one entry ordinarily, two for a claim about a merge. This listing reports a
merge-shaped claim in full rather than refusing one, unlike `extract --claim`, which has no
mainline to pick from and refuses such a claim outright — a listing has no such need to pick one.
**`"claims"` is never empty:** this listing's membership requires a held claim, so every entry it
reports has at least one. A patch no held claim
names is `"unclaimed"` (`show`'s own `state`), not pending, and does not appear in this listing at
all. `patches[]` is ordered by patch id, the same order the text form already uses; each entry's
own `claims[]` is ordered by claim id. **The text form itself (the count line and one id per line)
keeps its exact shape** — only the membership changed.

```json
{"schema_version": "sync-pending-v1", "patches": [{"patch_id": "<id>", "claims": [{"claim_id": "<id>", "target_block_id": "<id>", "parent_block_ids": ["<id>"], "signature": {"outcome": "sound", "key_id": "<key-id>"}}, {"claim_id": "<id>", "target_block_id": "<id>", "parent_block_ids": [], "signature": {"outcome": "invalid", "key_id": "<key-id>", "reason": "<the refusal's own text>"}}]}]}
```

**Reading a point: `tree`.** `tree` lists the files present at a point — a ref, or a bare block id — with
each one's mode, exact size and whether it is text or binary, by the same classification `checkout
--patch-plan --format json` reports. It reads received refs. Without `--ref` it lists the current branch,
and an unpublished current branch lists nothing, exit `0`; an explicit absent ref refuses. `--prefix`
filters by whole path components; the replay is whole-tree either way. `--format json` is
`tree-listing-v1`. See [Reading a point](../guide/tree-and-cat.md).

**Reading a point: `cat`.** `cat` writes one file's bytes at a point, text or binary, to stdout or to
`--output`. The content is resolved in full before anything is written, so a refusal writes nothing.
`--max-bytes` bounds what is written — exactly, and all or nothing — but **not memory**, which is the shared
replay's own cost; `--output` refuses an existing file without `--force` and any path inside `.prikk/`,
and is written to a temporary sibling then renamed into place. **Binary content refuses a terminal**, naming
`--output`; text is written. A path that is not a file at the point refuses with `path <p> does not exist at
<point>`. `--format json` is `path-content-v1`: the same fields as one `tree-listing-v1` entry, and no bytes.

**Writing a point into a new folder: `extract`.** `extract --ref <ref|block-id> <dir>` writes every file
at a point into a new directory that does not exist yet, no repository history and no worktree state, the
same shape `git archive | tar -x` produces. `<dir>` must resolve outside this repository's own root
(`.prikk/` included) — compared by identity, never by spelling, so a relative path, a `..` that lands back
inside, a symlinked parent, or a differently-cased or short-name spelling of the root on Windows all refuse
just as a plain path inside the worktree does; a path outside the repository, including a sibling
directory whose name merely starts with the repository's own name, is unaffected. The reason is the same
one `commit --from-worktree` already gives for its own worktree content: a copy left inside the repository
would be picked up as worktree content by the very next commit. Unlike `cat`/`tree`, **it anchors only
where a writer would**:
the same admitted-anchor rule `checkout`'s own worktree writes use, never the weaker rule that lets `cat`
and `tree` display any snapshot that merely decodes correctly. A `remotes/...` ref under a key this
repository has not adopted therefore always replays fully from its own genesis; past 63 blocks with no
admitted snapshot in reach, `extract` refuses by name, giving the distance and `--allow-full-replay` to
read it anyway. Every file whose content has not changed since the admitted anchor streams straight from
its stored blob id, nothing is held resident past the one file's own size, and a path renamed after the
anchor still streams from that same id, at its new path. The target must not already exist; a leftover
`<dir>.prikk-extract-tmp` from an interrupted run refuses by name rather than being reused or deleted; the
whole write is atomic (a temporary sibling, then one no-replace rename), and every file and directory is
synced before it, so a crash never leaves a target with missing or partial content. `provisional: yes|no`
says whether an admitted anchor was actually found and used, or the replay fell back to genesis instead.

**`--plan-only`** reports the point's own file count, total bytes, `largest file: N bytes` and
`provisional`, and writes nothing at all — no target, no temporary sibling. **`--max-bytes N`/`--max-files
N`** refuse the whole point, by name, before the first byte is written — learning the point's own total
size this way costs what it costs (each file not already resident is read once to measure it), the same
cost a real write already pays; the *pre-walk's* own bound (refusing a point whose nearest admitted
snapshot is unreachably far back) is the separate, cheap guard against unbounded history depth, checked
before a single Patch or Blob is read, never relaxed by these flags. **`--format json`** (`extract-report-v1`) carries `file_count`, `total_bytes`, `largest_file_bytes` (the single
largest entry's own byte length, always present, the same number the text form's `largest file: N bytes`
line gives), `provisional`, `written` (whether this call actually wrote the target), `point` (the ref name or
the block/claim id given, the one fixed key `tree`'s own `tree-listing-v1`/`cat`'s own `path-content-v1`
both use), `point_kind` (`"ref"`, `"block"`, or `"claim"`), `target_block_id`, `claim_parent_block_id`
(always present; `null` unless `point_kind` is `"claim"`, in which case it is the *named* claim's own
declared parent — the block its own patches would be sealed *onto*, not `target_block_id`, which is the
block the claim *recognizes*), `chain`, and `chain_signature` (both described below) — the same
hand-rolled, `schema_version`-named convention those commands already use. **`--claim <claim-id>`**
(mutually exclusive with `--ref`) previews a pending `RecognitionClaim` — the same id `sync seal --claim`
takes — as if its own patches were sealed onto its own declared parent, without sealing anything: useful
to inspect what a claim would produce (its size, or its content) before deciding to accept it.

When the named claim's own declared parent is not itself a real, reachable block, `extract --claim` walks
back through whichever other held claim, if any, recognizes that parent block, and so on, until it reaches
a block that genuinely is reachable from some branch or received history this repository holds — or until
the walk cannot continue at all, which refuses `claim-parent-not-sealed` exactly as a parent this
repository has simply never received does. Every link the walk visits is checked the same way: a claim
naming more than one parent (one built from an ancestor `Merge` block) is refused by name, at any position
in the chain, not only the one given on the command line — there is no field in any claim saying which
parent is mainline, so this is refused rather than guessed. When two or more held claims recognize the
same block the walk needs to resolve, the one signed by a key this repository has adopted is used, and
every other candidate is named in the report rather than silently dropped; with none, or with more than
one, signed that way, the command refuses rather than guessing which is true. A chain that would revisit a
block it has already visited refuses as a cycle. Every link's own MAINTAINER signature is checked: one
that fails to verify against an adopted key, or that carries no MAINTAINER signature at all, refuses the
whole command — the claim named directly on the command line is checked exactly the same way as every
other link; a signature that simply names a key this repository has not adopted is not treated as a
failure and does not by itself refuse. A link whose own patches do not actually apply onto the state
before it refuses by name, distinctly from a damaged repository. `--claim` takes the identical pre-walk
bound as `--ref`/a bare block id, extended to the whole chain: the trusted distance from the real block the
walk finally reaches, plus every additional unsealed claim the walk had to pass through to reach it
together, must not exceed the usual bound, or the command refuses by name — naming both numbers — unless
`--allow-full-replay` is given. `extract`'s own peak memory,
whole command, tracks the largest single file it actually reads (roughly twice that file's own size, not
once — see [Current State](current-state.md)), not the point's own total content size; this holds the
same way whether the point is named by `--ref`, a bare block id, or `--claim` — `--claim`'s own own
derivation step (`prepare_extract_plan_for_claim`) costs a little more than the ref/block-id path's own
equivalent planning step (about 1.1–1.3x, measured), but that planning cost is small next to the one
shared, dominant cost every path pays: streaming the point's own largest file once. See
[Current State](current-state.md) for the measured figures, both the planning-only comparison and the
whole command. See [Reading a point](../guide/tree-and-cat.md).

**`chain` and `chain_signature`: the whole walked chain, for a claim.** `chain` is `[]` for a ref or
bare block id, which name no claim at all; for a claim, it carries every link the walk actually
used, oldest first, the *named* claim always last. Each entry is `{"claim_id", "target_block_id",
"parent_block_ids", "signature", "other_candidates"}` — the identical field names `sync pending`'s
own `claims[]` entries already use for the first four, with `signature` the identical
`{"outcome": "sound"|"unverifiable", "key_id"}` shape too (an `"invalid"` link anywhere in the
chain always refuses the whole command first, so one never appears here). `other_candidates` is
non-empty only on the one link chosen because it was the single adopted-key candidate among two or
more held claims recognizing that same block; each entry there is `{"claim_id", "signature"}` for a
candidate that was named but not used. `chain_signature` is the single weakest `"outcome"` across
every link in `chain` (`"sound"` only if every link is, otherwise `"unverifiable"`; `null` when
`chain` is empty) — a consuming tool does not need to scan the array itself to tell whether the
whole chain traces back to an adopted key. The text form prints the identical information: one
`link N: claim <id> (<outcome>)` line per entry, root to foot, an indented line naming any
`other_candidates`, and an explicit `unverified` line whenever `chain_signature` is
`"unverifiable"`.

**`refusal-v1`: a machine-readable refusal document.** Under `--format json`, *every* refusal `extract`
exits `1` for — never a usage error, exit `2`, which prints no document — writes exactly one JSON document
to **stdout** (never stderr; stderr's own text and the exit code are entirely unchanged by this). This is a
structural guarantee at `extract`'s own command boundary, not merely a property each refusal happens to
have today: a refusal reached by a path this document's own list does not yet name still gets a document,
classified by a fallback, rather than exiting `1` under `--format json` with nothing on stdout.

```json
{"schema_version": "refusal-v1", "command": "extract", "refusal": "<id>", "reason": "<id>|null", "message": "<the stderr text>"}
```

`refusal` is one of twenty stable, kebab-case ids: `over-max-bytes`, `over-max-files`,
`destination-exists`, `destination-inside-repository`, `temporary-sibling-exists`, `point-not-found`,
`claim-not-found`, `not-a-claim`, `claim-has-several-parents`, `claim-parent-not-sealed`,
`claim-signature-invalid`, `claim-does-not-fold`, `claim-chain-shared-block`, `claim-chain-cycle`,
`case-collision`, `no-checkpoint-in-reach`, `not-a-repository`, `outside-version-rule`,
`damaged-repository`, `io-failure`.
`claim-parent-not-sealed` means some link's own declared parent is neither itself reachable from any
branch or received history this repository holds, nor recognized by any other held claim — seal or
receive it first, then extract the claim again; this repository is not damaged.
`claim-signature-invalid` means some link's own MAINTAINER signature failed to verify against a key
this repository has adopted, or the link carries no MAINTAINER signature at all — the claim named
directly on the command line is checked exactly the same way as every other link in the chain the
walk builds from it. `claim-does-not-fold` means some link's own patches do not actually apply onto
the state the chain has built up to that point — that claim is unsound, not this repository. A
link whose own patch names a path outside this version's own rule (today: non-ASCII) is not this
case: it reads `outside-version-rule`, with the same "outside what this version of `prikk` accepts"
sentence `extract --ref` gives for the identical path (see "A path this version of `prikk`
does not accept is not damage", above) — a claim can be perfectly sound and still carry a patch a
newer `prikk` wrote.
`outside-version-rule` itself means the refusal's own cause was a path this version of `prikk`'s own
rule refuses (today: non-ASCII), never corruption — decided from the error's kind, never from its
message text, and given both by `extract --ref`
(a point whose own history replays a patch naming such a path) and by `extract --claim` (a chain
link whose own patch names one) for the identical underlying cause. An empty, absolute, too-long,
or otherwise ill-formed path is a different, permanently invalid cause — no future version of
`prikk` will ever accept it — and still reads `damaged-repository`, never this id.
`claim-chain-shared-block` means two or more held claims recognize the same block the walk needed to
resolve, with none, or more than one, signed by a key this repository has adopted — picking one
without that would be a guess. `claim-chain-cycle` means the walk would have revisited a block it
had already visited earlier in the same chain. `reason` is set only for `no-checkpoint-in-reach`,
naming which of three conditions the admitted-anchor search found in reach — **one cause at a
time**: the first blocker the search meets, not necessarily the only one. Fix it and run again; the
next refusal names the next cause, if any. In priority order: `signer-not-adopted` (a recorded
checkpoint whose signer this repository has not adopted — adopt the key), `not-recorded` (a
checkpoint in reach, never recorded by `verify`/`seal` — run `verify`), or `no-checkpoint` (none at
all — `--allow-full-replay`). Every other id carries `reason: null`. `message` is stderr's own text,
without its leading `error: ` prefix — the generic framing applied to every command's own error,
never part of the refusal's own text itself; the JSON document never paraphrases it.

**This is this project's own, reusable form for a JSON refusal** — `refusal-v1`'s shape
(`schema_version`, `command`, `refusal`, `reason`, `message`) was designed as a precedent other
commands adopt when next touched, not a one-off specific to `extract`. `bundle import` is its
second adopter (see above), for exactly three of its own refusals; `extract`'s own twenty ids
above are unaffected. `trust maintainer add` is a later adopter (next paragraph).

**Trusting a key: `trust maintainer add`, and a key id that is already bound.** `trust maintainer add
--key-id ID --public-key HEX` adopts one MAINTAINER key. A key id stays bound to the first public key it
was adopted with — **also after `trust maintainer remove`**, because the id travels in every signature and
a changed key under a familiar id is what trust on first use exists to catch — so offering a *different*
key for such an id refuses (exit `1`, `error: precondition not met: …`, nothing written). The message
says which state the id is in: *already adopted* (it is in `trust maintainer list`), or *not currently
adopted, but bound to the key it was first adopted with* (it was removed). Re-adding a removed id with its
**original** key works. Either way the way forward for a different key is a distinct id: its maintainer
signs under one (`prikk key generate --out <path>`, or `PRIKK_MAINTAINER_KEY_ID`) and you adopt it with
`trust maintainer add --key-id <that id> --public-key <its public key>`.

`--format json` prints one document and nothing else. On success it is `trust-add-v1`:
`{"schema_version": "trust-add-v1", "key_id": "…", "public_key": "<lowercase hex>", "newly_added": true,
"adopted_count": N}`. `newly_added` is `false` only for the idempotent re-add of an id that is already
adopted with the same key; re-adding a *removed* id with its original key is `true`, because the adopted
set gained the id (the key's own record was already kept and is not rewritten). `adopted_count` is the
`adopted maintainer keys: N` line of the text form. On exactly the refusal above it prints a `refusal-v1`
document to stdout, with `"command": "trust-maintainer-add"`, `"reason": null`, and `refusal` one of two
stable ids: `maintainer-key-id-adopted-with-different-key` (the id is adopted now) or
`maintainer-key-id-bound-to-removed-key` (the id was removed and only its first key remains on record).
Every other failure — a malformed id or key, a case-insensitive collision with another adopted id, a held
lock, a torn container tail — prints no document under the flag: stderr and exit `1` only, as without it.

**Removing a received pointer: `received remove`.** `received remove <remotes/…> [--plan-only]`
removes one received ref's own pointer from the received-index container — the same container
`bundle import`/`archive import` write and `cat`/`tree`/`checkout`/`extract` read at a bare
`remotes/…` point. **The objects it reached are not removed, only the pointer**: a stored `Block`,
`RefState`, `Patch` or `Blob` nothing else reaches any more is reported by `prikk verify` as an
**unreached object** (`unreached_objects`, with `object_type` one of `"block"`, `"ref-state"`,
`"patch"` or `"blob"`) and by `prikk doctor` as a `PRIKK-DOCTOR-UNREACHED-OBJECT` warning — a
non-blocking warning either way, never a failure (see `verify`'s own entry above). A rolled-back or
`received remove`d block's own patch (and the blobs its own operations name), a claimless accepted
patch, and a crashed or interrupted import's leftover patch and blob objects are counted too. An
object counts when no tip, no WAL record and no pending patch (with the blobs its operations name)
reaches it. A `Block`/`RefState`'s own publication trust is skipped while it stays
unreached (reclassified, and checked again, the moment something reaches it); a `Patch`/`Blob` has
no publication-trust concept to skip in the first place — for them this is purely a presence
record. The bytes stay either way; no command removes them yet. When `verify` also reports a
damaged object, the walk cannot read past it: objects reached only through the damaged one are
counted as unreached, and their publication trust is not checked, so the count can include objects
that are in fact reached. `verify` still fails on the damage itself (`verify/reachability.rs`,
`verify/objects.rs`). `--plan-only`
reports what would be removed — the pointer's own current target, or that there is nothing named —
writing nothing, but it still takes the container's lock for the duration of the read: the same
reason `compact`'s own `--plan-only` variants do — an operator acts on what a preview reports, and
the read should not go stale against a concurrent writer before it reaches the terminal; the lock
is released once the read is done. A real run saves the removal to the recovery log
unconditionally, naming the run id so `prikk doctor --recovery-restore <id>` can undo it, as long
as nothing has since written to the same slot (a later, sound re-import of the identical name makes
the restore refuse, rather than silently discarding it). Refuses a name that does not start with
`remotes/`, a name with no received pointer, and a damaged received index, the same way `compact
--received-index` already does.

**Comparing: `diff`.** `diff` shows what changed between two states. A bare `prikk diff` compares the current
branch's tip (with any queued commits) against the **worktree** — exactly what `commit` would author, so `commit`
then `diff` is empty again; `--from <point>` compares that point against the worktree; `--from` and `--to`
together compare two points, each a ref (a received ref is read, not adopted) or a bare block id. A lone `--to`
is a usage error. Each differing path has one status: `added`, `deleted`, `modified`, `renamed`, `mode` or
`binary`; a new worktree file is `added`, with its content, and ignored paths never appear. Text changes carry
unified hunks with 3 lines of context, and a last line without a newline is marked as `diff -u` marks it. A
rename is reported only when history (or a live `prikk mv`) **declares** one, never inferred from similar
content. A binary file shows ids and sizes and never its bytes; `prikk cat` reads either side. A worktree path
`commit` would refuse is named under `unsupported_paths` with `commit`'s own refusal and no content. The line
diff is shortest within a work bound, above which the entry says `minimal: false` and its hunks, still valid,
are larger. `--path` (repeatable, exact) filters the output, and the work is whole-tree either way. Two states
that are the same give `no differences` and exit `0`; `diff` writes nothing. An unsupported operation, or a
blob that is missing or does not recompute, in either history fails the whole call. `--format json` is
`diff-report-v1`; see [Comparing](../guide/diff.md).

**Size bounds on incoming artifacts.** RFC 158 Stage A: `bundle import`, `bundle preview`, `bundle
verify`, `sync compare`, `sync build` and `sync accept` all read a file arriving from outside the
repository, and every one of them refuses **before reading it** when its size is over the bound —
on the open file's own metadata, so nothing large is allocated to discover it is large, then
enforced again while it streams, so a lying declared size is still caught. Every refusal names what
was too large and its size, the bound applied (in bytes and the nearest binary unit), where that
bound came from, and how to change it; none of them says "malformed" or a bundle is "persisted"
data, since neither is true of an oversized but otherwise well-formed input.

Four size bounds (three overridable, the have-list fixed) and three count bounds exist — six
variables in all (DC-86), none previously documented. The size bounds:

| bound | default | applies to | override |
|---|---|---|---|
| bundle | 256 MiB | `bundle import`/`preview`/`verify` | `PRIKK_BUNDLE_MAX_BYTES` |
| sync exchange artifact | 256 MiB | `sync accept` | `PRIKK_EXCHANGE_MAX_BYTES` |
| sync summary | 16 MiB | `sync compare` | `PRIKK_SYNC_SUMMARY_MAX_BYTES` |
| have-list | 64 MiB | `sync build --have` | fixed — no override exists |

A bundle and a sync exchange artifact also each declare a count of objects they carry (bundles) or
of patches/blobs/author keys/claims (exchange artifacts), bounded by `PRIKK_BUNDLE_MAX_OBJECTS` and
`PRIKK_EXCHANGE_MAX_OBJECTS` respectively (default 100,000 each), and the sync summary's ref count
by `PRIKK_SYNC_SUMMARY_MAX_REFS` (default 100,000).

**The per-object bound** applies to `bundle import`, `bundle preview`, `bundle verify` and `sync
accept` — everything that decodes objects from outside, never to a repository's own `commit`, which
is not bounded by default. It measures an object's *encoded* size as it travels — its content plus
a header — checked on the length prefix before the object's bytes are copied or decoded. **So a
file of exactly the bound you set refuses:** the header adds **69 bytes** (measured, for text and
binary files alike, and pinned by a test), so a file of N bytes needs a bound of at least N + 69.
The refusal names the size it saw, which is that N + 69. Default: 256 MiB, the same as the total
bundle/exchange bound, so it is inert at the defaults — it only acts once an operator lowers it.
Resolved highest-precedence first: `--max-object-bytes N` on the command itself (on `bundle import`,
`bundle preview`, `bundle verify`, and `sync accept`), else the repository's own
`incoming.max-object-bytes` in `prikk config` (not available to `bundle verify`, which opens no
repository), else the default. The bound is never taken from the input it bounds — nothing inside a
bundle or exchange artifact can raise it, and nothing an import or accept writes can reach
`.prikk/config`.

**All three bundle-reading commands now stream a regular file** rather than buffering it whole
before the store sees a byte, but by different amounts, because each needs a different amount held
in memory while it works:

- **`bundle import`** stays an open file the store reads and re-reads in place, deciding and
  writing one object at a time; its own memory follows roughly the size of the largest single
  object the bundle carries, not the whole bundle (measured, one 16 MiB object: about four times
  the object's own size).
- **`bundle verify`** decodes one object at a time and keeps nothing past its own turn, the same
  shape as `bundle import` — its own memory also follows roughly the largest single object, a
  little lower than `import`'s own figure since it never writes (measured, one 16 MiB object:
  about 2.3 times the object's own size, down from about 3.3 times before).
- **`bundle preview`** reads an object by seeking to its own range too, and never keeps a decoded
  `Blob` past the one call that read it (a `Blob` is file content, not closure metadata, so
  holding every one read would rebuild the whole bundle in memory one blob at a time). What is
  left is the comparison's own real working set: for one object, that is the object itself
  (measured, one 16 MiB object: close to `import`/`verify`'s own figure on the identical bundle);
  for a bundle that creates many files at once, the comparison genuinely holds each file's own
  content once while it decides what changed, so its own peak still grows with how much a bundle
  changes, not with the bundle's own size on disk (a mixed 158 MiB bundle peaks around 162 MiB,
  down from about 640 MiB before 0.55.0's streaming reads, where `import`/`verify` on the
  identical bundle peak under 15 MiB).

A pipe or FIFO input to any of the three is unchanged: a non-seekable input cannot be read twice,
so it is still read into memory whole before anything else happens.

**`prikk config`** is a repository's own small, durable settings file at `.prikk/config` — never in
the worktree, so a checked-out or imported file can never set one. One `key = value` per line; blank
lines and `#` comments are allowed. An unknown key, a duplicate key, or an invalid value (zero,
negative, non-integer) each refuse, naming the line, never a silent fall back to the default. The
only key today is `incoming.max-object-bytes`. `get`/`list` print the effective value and its source
(`(default)` or `` `.prikk/config` ``); `set` validates then writes atomically; `unset` restores the
default. **`set` rewrites the whole file and `unset` removes it, so a comment you wrote by hand — which
the format allows — is dropped without a word:** keep notes somewhere else. An older `prikk` build simply does not read this file — it has no effect on that build's
own behavior, and no error results from a repository holding one.

**Exit codes.** `0` — the operation succeeded and did what was asked. `1` — operational failure:
verification findings, an integrity failure, a refusal, a dirty worktree. `2` — usage error: an
unknown argument, a missing required flag, a duplicate flag. Graded verification results are in
`prikk verify --format json`, not in the exit code.
