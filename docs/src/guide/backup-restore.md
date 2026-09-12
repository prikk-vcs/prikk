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
objects: 4
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
objects: 4
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
objects: 8
author key material: 1 included (continuity only, not a trust decision)
repository format: 6
tool version: ...
note: this bundle contains one ref's closure only -- other refs in the source repository, if any, are not included, and this bundle makes no claim about them
wrote ../backup.bundle
```

`objects: 8` now (two RefStates, two blocks, two patches, two blobs) — `--force` was necessary
because `../backup.bundle` already existed from the first export.

## A bundle is one ref

`bundle export` takes exactly one `--ref`. A repository with several branches needs a bundle per
branch — nothing collects them for you, and the manifest's own printed note above says so in every
bundle you make, not only here.

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
objects: 8
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
objects: 8
new objects: 8
author key material: 1 recorded (continuity only, not a trust decision)
note: no local ref was created or advanced, and no MAINTAINER key was trusted; run `trust maintainer add` to trust the sealing key, then `merge` to incorporate this history
```

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
object items: 8 scanned, 0 failed
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

Turning `remotes/heads/main` into your own working `heads/main` is an ordinary `merge` from here —
outside this page's own scope, since bundle export/import add no "pull" concept of their own.

## What this proves, stated precisely

- **Corruption is detectable.** Every object's id is a hash of its own bytes, and `bundle verify`
  confirms every reference inside the bundle resolves to an object whose recomputed id matches.
  Content addressing works the same way a downloaded release's checksum does: it proves the bytes
  are intact and internally consistent, not who produced them.
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
- **This is not release-signer authority.** `release-signers.toml` is a separate, still-empty
  mechanism ([Release, Versioning, and Compatibility § Core Caveats](../reference/release-compatibility.md#core-caveats))
  governing official prikk releases, not repository-local maintainer trust — trusting a maintainer
  key here says nothing about who built the `prikk` binary you are running.

## Limits

- **One ref per bundle.** Multi-ref export does not exist; back up each branch you care about
  separately.
- **No rehearsed restore across a repository-format change.** This page exercises ordinary export
  and import on the current format; migrating a repository from an older on-disk format via bundle
  export/import, as `layout.rs`'s own retired-format messages describe, has not been exercised end
  to end as part of this page.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| A bundle carries one ref's own `RefState` plus every object reachable from its target block back to genesis; the active WAL is not part of that walk. | [`bundle.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/bundle.rs), [`worktree_patch.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_patch.rs) |
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
