# Backup and Restore

`prikk bundle export`/`verify`/`import` (DC-78) are what back up and restore a repository today.
This page is the recovery procedure: make a backup, check it later without touching anything, and
restore it — plus, just as important, what that restore does and does not establish about the
history you get back. See [Security and Signing Setup](security-setup.md) for the maintainer key
setup this page's `trust maintainer add` step assumes.

Prikk is early implementation software. Work through this page with throwaway content, the same
example key material [the tutorial](tutorial.md) uses — never real seeds.

## Set up a repository to back up

If you have already worked through [the tutorial](tutorial.md), skip to
[Make a backup](#make-a-backup) — this is the identical setup, condensed. Each refusal below is
explained there, not repeated here.

```sh
mkdir source-repo && cd source-repo
prikk init
export PRIKK_AUTHOR_KEY_ID="dev-author"
export PRIKK_AUTHOR_SEED_FILE="$PWD/author.seed"
export PRIKK_MAINTAINER_KEY_ID="dev-maintainer"
export PRIKK_MAINTAINER_SEED_FILE="$PWD/maintainer.seed"
prikk trust maintainer add \
  --key-id "dev-maintainer" \
  --public-key "a00899dfd3357aee69729405913f9324dfc033cec04a2215239eda64ae6d9d91"
echo "hello prikk" > readme.txt
prikk commit -m "genesis"
prikk seal --allow-no-audit
```

```
initialized Prikk repository at /path/to/source-repo/.prikk
trusted maintainer key: dev-maintainer
adopted maintainer keys: 1
recorded worktree patch in active WAL
baseline ref: heads/main
patch id: ...
WAL sequence: 1
operations: 1
referenced blobs: 1
...
  create-file readme.txt
...
sealed active WAL into block
patches: 1
block id: ...
heads/main RefState: ...
...
```

## Make a backup

A backup is one ref's sealed closure — the ref's own `RefState` plus every object reachable from
its target block back to genesis:

Run this from inside `source-repo`, writing the backup one level up so it survives independently
of the repository it came from:

```sh
prikk bundle export --ref heads/main --output ../backup.bundle
```

```
exported heads/main
tip block: ...
objects: 5
author key material: 1 included (continuity only, not a trust decision)
repository format: 6
tool version: ...
note: this bundle contains one ref's closure only -- other refs in the source repository, if any, are not included, and this bundle makes no claim about them
wrote ../backup.bundle
```

Running this again refuses rather than silently replacing the file:

```sh
prikk bundle export --ref heads/main --output ../backup.bundle
```

```
error: refusing to overwrite existing file at ../backup.bundle (pass --force to overwrite it intentionally)
```

Pass `--force` when overwriting is actually what you want.

## Only sealed history is backed up — this is the limit that costs the most

**`commit` records a patch in the active write-ahead log; only `seal` publishes it onto a ref.**
`bundle export` walks the ref's own published block ancestry — the active WAL is not part of that
walk. If you commit and do not seal before exporting, that commit is not in the bundle, and nothing
about the export command tells you so.

This is not a hypothetical. Still inside `source-repo`, commit again without sealing:

```sh
echo "not yet sealed" > notes.txt
prikk commit -m "notes, not yet sealed"
prikk bundle export --ref heads/main --output ../backup-after-unsealed-commit.bundle
```

```
exported heads/main
tip block: ...
objects: 5
```

**The tip block and object count are identical to the first export.** The second commit is not an
error, not a warning, and not present — the bundle is byte-for-byte a backup of the state before
that commit ever happened. Before exporting, check whether anything is queued:

```sh
prikk status
```

```
...
active WAL records: 1
trailing partial WAL bytes: 0
heads/main RefState: ...
queued patches: 1 targeting heads/main
...
```

`queued patches: 1` is the tell. Seal first, then export — now `backup.bundle` covers both
generations:

```sh
prikk seal --allow-no-audit
prikk bundle export --ref heads/main --output ../backup.bundle --force
```

```
sealed active WAL into block
patches: 1
block id: ...
heads/main RefState: ...
...
exported heads/main
tip block: ...
objects: 9
author key material: 1 included (continuity only, not a trust decision)
repository format: 6
tool version: ...
note: this bundle contains one ref's closure only -- other refs in the source repository, if any, are not included, and this bundle makes no claim about them
wrote ../backup.bundle
```

`objects: 9` now (two RefStates, two blocks, two patches, two blobs, and the snapshot the first block carries as a checkpoint) — `--force` was necessary
because `../backup.bundle` already existed from the first export.

## A bundle is one ref

`bundle export` takes exactly one `--ref`. A repository with several branches needs a bundle per
branch — nothing collects them for you, and the manifest's own printed note above says so in every
bundle you make, not only here.

For every ref, every object, and the trust material in one file, see
[The Archive Format](../reference/archive-format.md) instead: `prikk archive export <file>` carries
the whole repository, verbatim. `prikk archive verify <file>` reads one back and reports whether it
is internally consistent, needing no repository and writing nothing. `prikk archive import <file>
[--adopt]` lands every ref the archive carries into a fresh (or existing) repository: by default
under `remotes/<ref>`, the same received-pointer shape `bundle import` already produces, or as a
real local branch under `--adopt`, when it is absent locally, signed by a key this repository has
adopted (`trust maintainer add`), and verified — every other ref still lands under `remotes/`, with
the reason named.

**With prikk 0.51.0, run `archive verify` on every export before relying on it** (fixed in 0.52.0;
see [the limitation](../reference/current-state.md) for the two shapes and why).

**A repository holding any file over 256 MiB cannot be archived.** `archive export` (and
`bundle export`, for a ref that reaches such a file) refuses up front, naming the over-bound
object, and writes nothing — the same per-object bound `archive import`/`bundle import` already
enforce when reading one back. A copy of this repository's own `.prikk/` directory is the real
backup for a repository in this state.

**If the archive's own exported local ref and one of its own carried received pointers would land
at the same `remotes/<ref>` name** — because the source repository itself once `bundle import`ed
something for that same ref — the exporter's own ref wins the name; the colliding received pointer
is not landed, and the import's own report names it and the `RefState` it would have landed (its
objects are still imported either way). A successful `--adopt` lands the exporter's own ref as a
real local branch instead, so there is no collision at all in that case.

**If `archive import` is killed or lost partway through**, `prikk doctor` names it
(`PRIKK-DOCTOR-INTERRUPTED-IMPORT`) and its id, with two ways out: run the same import again (it
resumes from wherever it stopped), or `prikk doctor --cancel-import <id>` to restore the repository
to what it was before the import ever started. If the process was killed outright — a real
`SIGKILL`, a lost machine, not a command that returned an error on its own — its lock files are
still on disk (nothing steals a lock automatically: see `prikk unlock`'s own guide), and `doctor`'s
own message says so, naming `prikk unlock` *first*, ahead of resume or cancel. The sequence is
always the same order: `prikk doctor` to see what is true, `prikk unlock --lock <path>` for each
lock it (or `prikk unlock` on its own) lists, if any are present, and only then the same import
again, or `--cancel-import`.

## Check a backup later, without restoring it

`bundle verify` reads a bundle file and reports whether it is structurally sound and internally
consistent. It needs no repository and writes nothing — safe to run against a backup sitting on a
shelf, from any directory. Leave `source-repo` first, to show it really does not need one:

```sh
cd ..
prikk bundle verify --input backup.bundle
```

```
bundle verifies: heads/main
RefState: ...
tip block: ...
objects: 9
author key material: 1 present (continuity only, not a trust decision)
repository format: 6
tool version: ...
note: this bundle contains one ref's closure only -- other refs in the source repository, if any, are not included, and this bundle makes no claim about them
note: this checks structural and internal consistency only -- no signature is cryptographically verified (a standalone bundle carries no trust material to check one against), and this bundle's own author-key section is recorded here, never independently verified, the same as at import. A verified bundle is not yet a trusted one -- import it and run `prikk verify` for that.
```

## Restore

Restoring means importing into a repository and then checking the result — import alone is
deliberately not enough to trust what arrived.

```sh
mkdir restored-repo && cd restored-repo
prikk init
prikk bundle import --input ../backup.bundle
```

```
initialized Prikk repository at /path/to/restored-repo/.prikk
received remotes/heads/main
RefState: ...
objects: 9
new objects: 9
author key material: 1 recorded (continuity only, not a trust decision)
note: no local ref was created or advanced, and no MAINTAINER key was trusted; this repository now holds the received history at remotes/heads/main, which `prikk log --ref remotes/heads/main` and `prikk show` read, and `prikk verify` checks once `trust maintainer add` trusts the sealing key
```

**Restoring into a repository that already holds some of the history** stores nothing twice. An object
it already holds under the same signatures is counted but not written; the same object under an
additional signature — a copy another maintainer sealed — merges into the stored one in a format-7
repository, reported as `objects gaining signatures: N`, and is refused in a format-6 one (see
[Sync](sync.md)).

**Import records material; it does not create trust.** The imported history lands as
`remotes/heads/main`, not `heads/main` — no local ref moved, and the maintainer key that sealed it
is not yet trusted here. Trust it explicitly, the same key id and public key
[Security and Signing Setup](security-setup.md) describes adopting:

```sh
prikk trust maintainer add \
  --key-id "dev-maintainer" \
  --public-key "a00899dfd3357aee69729405913f9324dfc033cec04a2215239eda64ae6d9d91"
```

```
trusted maintainer key: dev-maintainer
adopted maintainer keys: 1
```

Now confirm the restore:

```sh
prikk verify
```

```
verified repository: /path/to/restored-repo/.prikk
...
object items: 9 scanned, 0 failed
...
publication trust issues: 0
sealed blocks: 2
sealed-block ...: dev-maintainer
sealed-block ...: dev-maintainer
...
received refs: 1
received-ref remotes/heads/main: ...
```

`verify` is the same command, unmodified, that checks any locally-authored repository — the bundle
format adds no separate verification path. A restored copy is only as checked as this command says
it is; import's own report is not a substitute for running it.

**What a restore gives, and what it does not.** Measured on the binary, in a fresh repository after
`bundle import`:

- **It gives verifiable history at `remotes/heads/main`.** `prikk log --ref remotes/heads/main` lists the
  received blocks, `prikk show <block>` reads their patches and content, and `prikk verify` checks
  every object and names the key that sealed each block. With no maintainer key trusted, `verify` reports
  publication-trust issues and exits non-zero; after `trust maintainer add` for the sealing key, it passes.
- **It does not give a working branch.** None of these turn the received ref into your own `heads/main`
  today: `prikk branch create heads/main --from remotes/heads/main` and `prikk checkout --patch-plan` or
  `--patch-materialize --ref remotes/heads/main` refuse (`remotes/heads/main is a received ref, and this
  command does not accept received refs`), so no worktree is written; and `prikk merge --into heads/main
  --from remotes/heads/main` refuses in a fresh repository (`ref heads/main does not exist in this
  repository`), even with the sealing key trusted. The refusal names what does read a received ref: `log`,
  `merge-evidence`, `merge-plan` and `bundle preview`.
- **It cannot be passed on.** `prikk bundle export --ref remotes/heads/main` refuses, so a restored copy
  is not itself a backup source.

A `bundle import` restore is therefore an intact, checkable copy of the history, not a repository you
continue working in that way. `prikk archive export <file>` writes a whole repository, verbatim, into
one file (RFC 155) — see [The Archive Format](../reference/archive-format.md). `prikk archive verify
<file>` reads one back and checks structure, reference closure, and every signature against the
archive's own carried key material, reporting *internally consistent*, never *trusted* — it needs no
repository and writes nothing. A signature by a key the archive does not carry (for instance, a
received branch the source repository itself never trusted) is named, not refused on: trust is
always the receiving repository's own decision, never the archive's. `prikk archive import <file>
--adopt` is the landing step that turns a
carried branch into a working one: absent locally, signed by an adopted key, and verified, it becomes
a real local branch; every other ref still lands under `remotes/`, exactly like `bundle import`'s own
received pointer, with the reason named.

## What this proves, stated precisely

- **Corruption is detectable.** Every object's id is a hash of its own bytes, and `bundle verify`
  confirms every reference inside the bundle resolves to an object whose recomputed id matches.
  Content addressing works the same way a downloaded release's checksum does: it proves the bytes
  are intact and internally consistent, not who produced them.
- **A deletion carries the content it removed.** A bundle includes the file content each deleted node
  held, so a receiver can **read** that deletion in full — `prikk show` renders the content rather than
  `<unavailable blob …>`. Where nothing stored that content — a text file whose edits only ever travelled
  as spans — the exporter derives it by replaying the history and carries the result, checked against the
  id the deletion names. Before 0.43.0, exporting such a history refused outright.
  **Rolling that deletion back is a separate matter:** `rollback-preview` and `inverse-plan` work on a
  published local ref, and an imported history lands at `remotes/…`, so both refuse there
  (`remotes/heads/main is a received ref, and this command does not accept received refs`). The content is
  present for when the history is local.
- **`bundle verify` answers "is this file intact" without a repository or a restore.** Nothing is
  written; nothing needs to exist first.
- **`verify_repository` after import is what turns a restored copy into a checked one** — the same
  structural and authorship checks any repository gets, run against what was actually written.

## What this does not prove

- **No signature is cryptographically checked offline.** A standalone bundle file carries no trust
  material to check a signature against — `bundle verify` checks structure only. Authenticity is
  checked after import, by `verify`.
- **Authorship is trust-on-first-use, not identity.** See
  [Trust and Threat Model § Trust Roots and Roles](../reference/trust-threat-model.md#trust-roots-and-roles)
  for exactly what that does and does not establish — it is not restated here, so the two cannot
  drift apart.
- **A restored repository has no maintainer trust policy until you give it one.** `bundle import`
  adopts no key; `prikk verify` reports publication-trust issues until `trust maintainer add` is
  run for the key that sealed the history you received.
- **This is not release authority.** Releases of `prikk` itself are signed by the project's one
  maintainer key, and `release-signers.toml` is empty because no multi-signer policy exists yet
  ([Release, Versioning, and Compatibility § How prikk releases](../reference/release-compatibility.md#how-prikk-releases)).
  That is separate from repository-local maintainer trust — trusting a maintainer key here says
  nothing about who built the `prikk` binary you are running.

## Limits

- **One ref per bundle.** Multi-ref export does not exist; back up each branch you care about
  separately.
- **No rehearsed restore across a repository-format change.** This page exercises ordinary export
  and import on the current format; migrating a repository from an older on-disk format via bundle
  export/import, as `layout.rs`'s own retired-format messages describe, has not been exercised end
  to end as part of this page.
- **`bundle import`, `bundle preview` and `bundle verify` each refuse a bundle over a size bound
  before reading it**, and refuse any one object inside it over a separate bound you can set
  (equal to the total by default, so it only bites once you lower it) — see [Size bounds on incoming artifacts](../reference/commands.md#size-bounds-on-incoming-artifacts)
  for the defaults, the refusal wording, and how to raise or lower either one.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| A bundle carries one ref's own `RefState` plus every object reachable from its target block back to genesis; the active WAL is not part of that walk. | [`bundle.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/bundle.rs), [`worktree_patch.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/commit_boundary/worktree_patch.rs) |
| `bundle export` refuses an existing destination unless `--force` is passed, and writes atomically. | [`bundle.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/bundle.rs), [`durable_output.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/durable_output.rs) |
| The `PBNDL003` manifest states repository format, tool version, and that the bundle's scope is a single ref. | [`bundle.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/bundle.rs), [Release, Versioning, and Compatibility](../reference/release-compatibility.md) |
| `bundle verify` checks structure, framing, and closure resolution with no repository and no write; it performs no cryptographic signature check. | [`bundle.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/bundle.rs) |
| `bundle import` records objects and author-key material and creates a `remotes/<ref>` received pointer; it advances no local ref and adopts no maintainer key. | [`bundle.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/bundle.rs) |
| `verify_repository`, unmodified, is what checks a restored repository — the bundle format adds no separate verification path. | [`bundle.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/bundle.rs), [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs) |
| AUTHOR key material transported in a bundle is trust-on-first-use; it proves continuity of authorship, not identity. | [Trust and Threat Model](../reference/trust-threat-model.md) |
| Multi-ref export does not exist, and no rehearsed restore across a repository-format change has been performed. | [`bundle.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/bundle.rs), [rfcs/done/DC-44-MIGRATION-BACKUP-RESTORE-EVIDENCE.md](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-44-MIGRATION-BACKUP-RESTORE-EVIDENCE.md) |

## Provenance

This guide implements DC-44's last design goal (`bundle-manifest-handoff-v1.md`'s own follow-up,
`backup-restore-page-handoff-v1.md`). It is documentation-only and does not change CLI behavior,
bundle format, signing, trust, or repository state.

---

**This page's own command sequence is exercised by an automated test**
(`crates/prikk-cli/tests/dc44_backup_restore_page.rs`), which runs every command above, in order,
against the compiled binary, and asserts on the same output lines quoted here, including the
`--force` refusal and the unsealed-commit demonstration. A change to the CLI that alters any of
them fails that test, not just this page.
