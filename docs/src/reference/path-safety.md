# Path and Worktree Safety

This page is the authoritative current-state reference for Prikk's repository path validation and
worktree write-safety boundaries. It describes the current implementation and is grounded in the code,
released RFCs, and implementation status records listed in the anchor table at the foot of the page.

For physical repository layout and `.prikk/` authority boundaries, see
[repository layout and authority](./repository-layout.md). For trust and threat boundaries, see the
[trust and threat model](./trust-threat-model.md). For local lock and stale-lock behavior, see
[concurrency and locking](./concurrency-locking.md).

## Core Caveats

- Prikk is early implementation software and is not a production Git replacement.
- Repository paths currently use a conservative ASCII-only subset.
- Unicode NFC normalization is not implemented; non-ASCII repository paths are rejected.
- Cross-platform conservative checks are enforced even on Unix, including Windows reserved names and
  case-insensitive collision rejection.
- The collision-rejection rule is ASCII case-folding only (`to_ascii_lowercase`), applied uniformly
  to repository paths, branch ref names, tag ref names, and maintainer trust key ids (DC-72). It is
  **not** Unicode normalization: an NFC-composed and NFD-decomposed spelling of the same visible name
  are different byte sequences and are not folded together. Repository paths cannot reach this case
  today because non-ASCII repository paths are rejected outright (previous bullet); branch and tag ref
  names have no such ASCII restriction, so an NFC/NFD pair there is a live, recorded, un-rejected
  collision. Locale-dependent case rules (Turkish `İ`/`i`, German `ß`/`SS`) are outside ASCII folding
  for the same reason. Closing this needs a normalization dependency prikk-store's dependency
  allowlist does not currently permit (`tools/release-policy/src/boundary/placement.rs`).
- Repository-path collisions are rejected at `seal`, not at `commit` (DC-72) — `commit` records a
  case-colliding pair into the active WAL without error; `seal` computes the full state root over all
  live paths and rejects there. Nothing enters sealed, verifiable history either way, but the
  rejection surfaces later than the action that introduced it. Recorded as a known ergonomic gap, not
  fixed — moving the check earlier is a separate change to the commit path.
- Branch and tag ref-name collisions, and maintainer trust key id collisions, are rejected only when
  the name is first created (no prior published state for that exact name) — an ordinary pointer
  update to an already-published ref does not re-scan every other ref.
- Symlink authoring and symlink materialization are deferred.
- Current materialization safety is check-then-write. It is not an `openat`/`O_NOFOLLOW` design, not a
  canonical realpath proof, and not a race-free guarantee under concurrent worktree modification.
- Repository *mutation* is exercised by project gates on Linux, macOS, and Windows (DC-87 Stage 2).
  Windows' anchoring guarantee is weaker than Linux/macOS in one stated way — see
  [platform support](./platform-support.md) for the exact gap and which of the `DurabilityContract`
  methods are held, weaker, or documented no-ops there. Read-only commands are CI-gated on macOS
  and Windows too — see [platform support](./platform-support.md).
- Stable path-format policy, path-policy configuration, Git path compatibility, stable
  repository-format migration, and complete checkout semantics remain deferred.

## Repository Path Shape

Prikk's `RepoPath` is a validated repository-relative path string. The current accepted shape is:

- non-empty;
- ASCII only;
- repository-relative, with no leading `/`;
- slash-separated with `/`;
- made of non-empty components;
- not targeting the top-level `.prikk` metadata directory;
- free of the rejected component forms listed below.

`RepoPath` is a logical repository path, not a host path. Store code joins it to the repository root
only after validation.

## Rejected Path Forms

The current validator rejects:

- empty paths;
- absolute paths that start with `/`;
- backslashes;
- colon characters;
- non-ASCII bytes;
- control bytes `0x00` through `0x1F` and `0x7F`;
- empty path components;
- a path component longer than 255 bytes;
- a total path longer than 1024 bytes;
- `.` and `..` components;
- `.prikk` as the first component, case-insensitively;
- components ending in a space or dot;
- Windows reserved component basenames: `CON`, `PRN`, `AUX`, `NUL`, `COM1` through `COM9`, and `LPT1`
  through `LPT9`;
- duplicate paths in a path set; and
- case-insensitive collisions in a path set.

### Length Caps

Added by RFC 125 §3: before this, `validate_repo_path` had no length check of any kind, so an
over-long path entered signed history and only failed later — as a raw OS error, not a clean
refusal — at checkout on a repository that had already verified clean.

- **255 bytes per component** — POSIX `NAME_MAX`, the per-component ceiling shared by Linux, macOS,
  and Windows' own NTFS component limit (255 UTF-16 code units, the same count as bytes here since
  repository paths are ASCII-only). A component longer than this cannot exist on any of the three
  platforms this project mutates on ([platform support](./platform-support.md)), regardless of
  where in the tree it sits or how deep the worktree root is.
- **1024 bytes total** — macOS's `PATH_MAX` (`sys/syslimits.h`), the strictest of the three
  platforms' hard total-path limits: Linux's is 4096, and Windows' legacy default (`MAX_PATH`, 260)
  is a separate, narrower gap that depends on the worktree root's own depth too and cannot be
  bounded from a repository-relative length alone. Chosen as the floor so a path this validator
  accepts is materializable on Linux and macOS unconditionally, regardless of worktree root depth.
  **Not a guarantee against Windows' legacy `MAX_PATH`** — a sufficiently deep worktree root can
  still exceed it even for a path this validator accepts; that residual gap is a property of where
  a repository is checked out, not something a repository-relative length cap alone can close.

The Windows reserved-name check is matched on the component basename before the first `.`. It is not a
complete Windows path policy and does not include `COM0` or `LPT0`.

The `.prikk` rejection applies to the first component only. A later `.prikk` component is not rejected
by that specific validator rule. Worktree authoring separately skips the top-level `.prikk/` directory.

## Snapshot Manifest Paths

Snapshot manifests decode path bytes as UTF-8 text, parse each path through `RepoPath`, and then
validate path ordering and collisions. Manifest entries must be sorted by repository path. Duplicate
paths and case-insensitive collisions are rejected.

Snapshot entries also carry length-framed content bytes. The path-safety check does not inspect file
content; it validates where the content may be represented or materialized.

## Materialization Safety

Snapshot materialization is opt-in through `prikk checkout --snapshot-materialize`. It writes files only
from a validated snapshot manifest. Patch materialization is opt-in through
`prikk checkout --patch-materialize` and writes the supported patch replay result through the same
shared materializer.

For each materialized file, the current implementation:

- joins the validated `RepoPath` to the repository root;
- checks that the joined path lexically starts with the repository root;
- checks each existing parent directory with symlink-aware metadata and refuses symlink parents;
- refuses non-directory parent paths;
- checks an existing final target with symlink-aware metadata;
- refuses symlink targets;
- refuses non-file targets;
- leaves existing files unchanged when bytes already match;
- refuses to overwrite existing files with different bytes;
- writes new file bytes through the current atomic file-write helper; and
- never removes extra worktree files during ordinary snapshot or patch materialization.

This is intentionally conservative, but it is not complete symlink-escape protection. The containment
check is lexical rather than canonicalized realpath proof. Parent and target checks happen before the
write. A concurrent process that mutates the worktree between checks and writes is outside the current
guarantee.

## Deletion Safety

Patch deletion is a separate opt-in path:

```text
prikk checkout --patch-materialize-delete [path] [--ref REF]
```

The command removes only files that the replayed supported patch chain explicitly removed with a
`DeleteFile` operation. Before removal, Prikk checks the current worktree target with symlink-aware
metadata, refuses symlink targets, refuses non-regular targets, and requires the current bytes to match
the deleted file's old Blob precondition bytes.

Already-absent deletion targets are counted separately. Arbitrary untracked files are never deleted,
and general checkout pruning remains deferred.

## Worktree Authoring Safety

`prikk commit` enumerates regular worktree files, skips the top-level `.prikk/` metadata directory, and
validates identity-bearing paths through `RepoPath`.

The current authoring path:

- rejects symlink entries because symlink authoring is out of scope;
- rejects non-regular entries;
- rejects non-UTF-8 host paths before they can become repository paths;
- validates each repository-relative path through `RepoPath`;
- normalizes regular file modes into Prikk's supported mode representation; and
- rejects snapshot-only published baselines as worktree-authoring identity authority.

Worktree authoring does not infer renames. A move is represented as a deletion plus a creation in the
current supported authoring model.

## Ref and Tag Name Safety

`prikk branch create`, `prikk tag create`, and a branch's first `seal` (the moment that publishes a
ref with no prior published state) reject a new ref name that ASCII-case-folds to an existing ref
name other than itself. An ordinary pointer update to an already-published ref does not re-run this
check.

Branch names (`heads/...`) and tag names (`tags/...`) are folded and compared only within their own
namespace: `validate_local_branch_ref`/`validate_local_tag_ref` require the exact, case-sensitive
`heads/`/`tags/` prefix, so the two namespaces never fold into each other. `heads/Main` colliding with
`heads/main` is rejected; `tags/Main` alongside `heads/main` is not a collision.

Ref names have no non-ASCII restriction — unlike repository paths, a branch or tag literally named
`café` is accepted. Because the fold is ASCII-only, an NFC-composed and an NFD-decomposed spelling of
the same name are not recognized as colliding; see the ASCII-folding caveat above.

## Maintainer Trust Key Id Safety

A maintainer key id is a field inside a trust-container record, not a filesystem path component — RFC
102 Stage 5 replaced the earlier `{key_id}.pub`-under-`.prikk/trust/keys/maintainer/` storage, which is
what originally motivated checking it the same way a repository path is checked. The same checks are
still applied, now for their own reasons rather than a filesystem hazard:

- storage-safe character allowlist (ASCII alphanumeric, `-`, `_`) — kept as a conservative constraint on
  the id shape, independent of any current storage mechanism;
- Windows reserved device stem rejected regardless of host OS (`CON`, `PRN`, `AUX`, `NUL`, `COM1`
  through `COM9`, `LPT1` through `LPT9`) — this is the same check `RepoPath` uses, shared rather than
  duplicated;
- case-insensitive collision against every other *currently-adopted* key id is rejected — a semantic
  guard against operator confusion and ambiguous audit trails now, not a case-insensitive-filesystem
  workaround. A key id removed from the active policy no longer reserves its case-folded name.

`trust maintainer add` adds a new key id, confirms idempotently if it already matches, and refuses if it
conflicts with a different key under the same id (DC-78's TOFU enforcement) — re-adding an unchanged
`key_id` is not treated as colliding with itself. `trust maintainer remove` revokes a key id from the
active policy; the key's own material is retained internally, so a *different* key presented later under
the same id is still refused even after removal.

## Deferred and Not Promised

Still deferred: Unicode NFC normalization, non-ASCII repository paths, symlink authoring, symlink
materialization, full platform path matrix, Git path compatibility, path-policy configuration, stable
repository-format migration, complete checkout pruning, complete branch switching, production merge
execution, and race-free worktree mutation hardening.

Current checks are deliberately strict so future path policy can expand from a conservative baseline.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| `RepoPath` accepts only the current ASCII, repository-relative, slash-separated subset. | [`path.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-replay/src/path.rs), [`path/tests.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-replay/src/path/tests.rs), [DC-32](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-32-PATH-WORKTREE-SAFETY-REFERENCE.md) |
| The `.prikk` validator rule applies to the first component case-insensitively. | [`path.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-replay/src/path.rs), [repository layout](./repository-layout.md) |
| Duplicate paths and case-insensitive collisions are rejected. | [`path.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-replay/src/path.rs), [`snapshot.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/snapshot.rs) |
| Snapshot manifests decode UTF-8 path bytes, parse `RepoPath`, enforce sorted paths, and length-frame content bytes. | [`snapshot.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/snapshot.rs), [snapshot checkout guide](../guide/checkout/snapshot-checkout.md) |
| Snapshot materialization writes only validated snapshot entries and uses the shared safe materializer. | [`worktree.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree.rs), [snapshot materialization guide](../guide/checkout/snapshot-materialization.md) |
| Patch materialization writes supported replay results through the shared safe materializer. | [`patch_checkout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_checkout.rs), [patch materialization guide](../guide/patches/patch-materialization.md) |
| Materialization checks lexical root-containment and refuses symlink parents, symlink targets, non-file targets, and conflicting existing files. | [`worktree.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree.rs), [`path.rs` store adapter](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/path.rs) |
| Materialization writes use the current atomic file-write helper. | [`fsutil.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/foundation/fsutil.rs), [`worktree.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree.rs) |
| Patch deletion is opt-in, deletes only explicit replay deletions, and requires current bytes to match old Blob precondition bytes. | [`patch_checkout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_checkout.rs), [patch deletions guide](../guide/patches/patch-deletions.md) |
| Worktree authoring skips top-level `.prikk/`, rejects symlinks/non-regular entries, rejects non-UTF-8 paths, validates through `RepoPath`, and rejects snapshot-only baselines. | [`node_authoring.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/commit_boundary/worktree_patch/node_authoring.rs), [worktree patch guide](../guide/patches/worktree-patch.md) |
| Current trust/threat docs treat `.prikk` private paths and absolute host paths as sensitive diagnostics material. | [trust and threat model](./trust-threat-model.md), [patch algebra reference](./patch-algebra.md) |
| Repository-path collisions are rejected at `seal` (state-root derivation), not at `commit`. | [`state_root.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/state_root.rs), [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs) |
| Branch and tag ref names reject a case-insensitive collision against another ref in the same namespace, checked only at first publication. | [`refs/publication.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs/publication.rs), [`refs.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/refs.rs), [`dc72_path_safety_collisions.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/tests/dc72_path_safety_collisions.rs) |
| Maintainer trust key ids reject a Windows-reserved stem and a case-insensitive collision against another stored key id. | [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/foundation/layout.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), [`dc72_path_safety_collisions.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/tests/dc72_path_safety_collisions.rs) |

## Provenance

This reference implements DC-32 as a documentation-only extension of the current-state reference series.
It adds no code, schema, CLI behavior, checkout behavior, materialization behavior, worktree authoring
behavior, repository behavior, trust behavior, verification behavior, release semantics, or stable path
policy guarantee.

DC-72 (NFR-SEC-03 path-safety conformance) added the ref/tag-name and maintainer-key-id collision and
reserved-name checks this page now documents, and the ASCII-folding/seal-timing caveats above. That
work was code, not documentation-only; this page's provenance note is scoped to what DC-32 originally
contributed, not to every later increment that changed the behavior it describes.
