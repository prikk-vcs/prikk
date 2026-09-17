# Git → prikk: a vocabulary map, not a feature map

If you know Git, prikk's commands will look familiar and several of them mean something different.
`commit` does not publish. There is no `HEAD` and no staging area, though there is a current branch
that `branch switch` moves. `seal` has no
Git counterpart at all. Reading prikk's words as Git's words will make you wrong about several things
at once, and you will not notice, because nothing will error — the commands will simply do something
other than what you expected.

This page is a vocabulary map for a human, not a migration tool. It does not import Git history and
does not promise to; the closest active work on that is
[RFC 113](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/113-history-import-foundations.md),
which is a design for the import *contract*, and nothing implements it yet.

## Command correspondence

| Git | prikk | What differs |
|---|---|---|
| `git init` | [`prikk init`](../guide/tutorial.md) | Same idea: create the repository layout. |
| `git clone` | — | **Missing.** [`prikk bundle import`](../guide/backup-restore.md) reads a bundle file into an untrusted, unmerged `remotes/<ref>` pointer — it does not create a worktree or a local branch. There is no network transport at all: bundles and sync artifacts are files you move yourself. |
| `git fetch` | [`prikk sync have <ref>`](../guide/sync.md#the-loop-as-a-person-actually-runs-it) (you) → `prikk sync build` (them) → `prikk sync accept <file>` (you), or `prikk bundle import` | **File-based, and never merged.** What arrives is an untrusted `remotes/<name>` pointer. No sockets: you move the file yourself. |
| `git pull` | Fetch as above, then `prikk sync seal <ref> --claim <id>`, or `prikk merge-evidence` → `prikk merge-plan` → `prikk merge` | **No single verb, by design.** `sync seal` when the received patches extend your ref; the three merge steps when the histories diverged. |
| `git push` | — | **No counterpart.** You produce the artifact (`prikk sync build`, `prikk bundle export`) and the other side accepts it; prikk never writes into another repository. |
| `git add` (staging) | — | **Missing, deliberately.** There is no index and no staging area. `prikk commit --from-worktree` always considers the whole worktree (governed by `.prikkignore`, which is a flat list of literal path prefixes — no globbing, no negation, unlike `.gitignore`; see [Ignoring Worktree Paths](../guide/ignore.md)). You cannot stage part of a change. |
| `git mv` | `prikk mv <old> <new>` | **A declaration, authored into the next commit.** Moves the file and records the rename for the next `commit`; if `<old>` is already moved on disk (a shell `mv`), it records the declaration alone and touches no bytes. |
| `git commit` | [`prikk commit --from-worktree -m <message>`](../guide/patches/worktree-patch.md) | **Two-phase, not one.** `commit` only queues a signed Patch into a local WAL; nothing is published yet. See [Commit versus seal](#commit-versus-seal-a-two-phase-model) below. **The message is stored as signed evidence** since `0.32.0` — see [Messages are stored](#messages-are-stored-authors-and-dates-are-not).
| — | [`prikk seal --allow-no-audit`](../guide/patches/worktree-patch.md) | **No Git counterpart.** Publishes the queued WAL as a signed Block, moving the branch ref. Nothing is visible to `log` or a peer before this runs. |
| `git status` | [`prikk status`](../guide/status.md) + [`prikk worktree-status`](../guide/worktree-status.md) | Split in two: `status` reports the repository/WAL/queue state; `worktree-status` reports the worktree's own drift against the replay baseline `commit` would author against. Neither is `git status`'s single combined view. |
| `git log` | [`prikk log`](../guide/history.md) | Block and ref metadata only — target Block ID, RefState ID, kind, parent/Patch counts, rollback classification. **No commit message, author, or date is shown**, because none is stored yet (see below). |
| `git show` | [`prikk show <block-id\|patch-id>`](../guide/show.md) | **One block or patch, as its operations.** Prints each operation with the content it carries — a created file's text, an edit's old and new span — rather than a unified diff. Content comes from the patch itself; a block id additionally resolves edited nodes to their paths by one replay, which a bare patch id cannot. |
| `git ls-tree -r` | [`prikk tree [--ref <ref\|block-id>] [--prefix <p>]`](../guide/tree-and-cat.md) | **Files only, no directory entries.** Lists every file present at a ref or a bare block id, with mode, exact size, and whether it is text or binary; a binary file also shows its blob id. `--prefix` matches whole path components. `--format json` is `tree-listing-v1`. |
| `git diff` | — | **Missing.** The closest things today are `prikk checkout --patch-plan --format json --content-path <path>` (content at one point) and [`prikk worktree-status`](../guide/worktree-status.md) (changed names against the baseline, no content). A diff between two points is proposed in RFC 153 and not implemented. |
| `git branch` | [`prikk branch list`](../guide/faq.md) / `branch create heads/<name>` | List and create work the same way in spirit. |
| `git branch -d` | `prikk branch close heads/<name>` | **Diverged.** Closing marks the branch (a schema-2 `closed` RefState); nothing is deleted or reclaimed, and there is no reopen verb. |
| `git checkout <branch>` / `git switch` | [`prikk branch switch heads/<name>`](../guide/faq.md#how-do-i-switch-branches) | **Stricter.** Refuses a dirty worktree and unsealed work for another branch rather than carrying changes across; untracked files are never touched. See [A current branch, no `HEAD`](#a-current-branch-no-head) below. |
| `git checkout -- <path>` / `git restore` | [`prikk checkout --patch-materialize`](../guide/checkout/checkout.md) (and the sibling `--snapshot-*`/`--patch-*` flags) | Materializes files for one `--ref`, plan-first (`--plan-only`, `--snapshot-plan`, `--patch-plan`). No pathspec — it is whole-ref, not a partial restore. |
| `git tag` | [`prikk tag create tags/<name> --target <ref\|block> [-m <message>]`](../guide/history.md) | **The tag message *is* persisted** — unlike a commit message, `TagPayload` already carries one. |
| `git merge` | [`prikk merge-evidence`](../guide/merge-evidence.md) → [`prikk merge-plan`](../guide/merge-plan.md) → [`prikk merge --allow-no-audit`](../guide/merge.md) | Three explicit steps, not one. `merge` executes only a merge already proven confluent by evidence; a merge that is not refuses with a witness rather than producing a conflicted worktree. |
| `git rebase` / `git cherry-pick` | — | **Missing.** The closest thing to cherry-pick is `prikk sync seal --claim <id>`, which seals one already-*received* patch — it cannot take an arbitrary patch from your own history. |
| `git revert` | [`prikk rollback-preview`](../guide/rollback/rollback-preview.md) → [`prikk rollback-draft --append-inverse -m <message>`](../guide/rollback/rollback-draft.md) → [`prikk rollback-draft-verify`](../guide/rollback/rollback-draft-verify.md) → `prikk seal` | Four explicit steps producing a preimage-exact inverse Patch, rather than one command that also commits. `prikk inverse-plan` shows the unsigned inverse without writing anything. None of them inverts a `prikk mv` rename yet: a history containing one is refused. |
| `git stash` / `git submodule` / `git worktree` (multiple) | — | **Missing.** One worktree per repository; nothing analogous to any of the three exists. |
| `git gc` | [`prikk compact --pointer-index\|--received-index\|--trust-policy\|--all`](../guide/backup-restore.md) | **Diverged, not equivalent.** prikk's object model never deletes an object — `compact` reclaims stale *index and policy* records only. There is no object pruning of any kind. |
| `git fsck` | [`prikk verify [--format json] [--stop-on-first-error]`](../reference/integrity-recovery.md) | Broader: object/WAL/ref integrity plus three-valued publication-trust classification in one pass. |
| `git reflog` | — | **No user-facing verb.** Every ref update is a signed, append-only `RefUpdate` log entry internally, but nothing surfaces it to a command today. |
| `git config` | — | **Missing.** No config file or command exists. Keys are read from a per-user key directory by default, and the `PRIKK_*_SEED_FILE` / `PRIKK_*_KEY_ID` environment variables override it — see [Security and Signing Setup](../guide/security-setup.md#current-key-inputs). |
| N/A — no counterpart | [`prikk key generate`/`prikk key public`/`prikk key status`](../guide/first-run.md) | Draws a fresh Ed25519 seed from the OS CSPRNG, derives a public key from a seed already held, or reports whether this machine can sign and with which key, signing nothing. Git has no equivalent — an SSH key or GPG key is generated with a separate tool entirely. |
| N/A — no counterpart | [`prikk setup`](../guide/first-run.md) | Composes `init`, key generation for both roles, `trust maintainer add`, and the export lines into one command. Nothing in Git bootstraps signing identity this way — `git init` alone reaches a usable repository because Git commits are unsigned by default. |
| `git blame` / `git bisect` / `git grep` | — | **Missing.** None exists in any form. |
| `.gitignore` | [`.prikkignore`](../guide/ignore.md) | **Diverged, and narrower on purpose.** Literal repository-relative path prefixes only — no globbing, no negation, no per-directory files, and it binds only at `commit`'s worktree walk and `worktree-status`'s scan, never at replay or verification. |
| `git remote add` / remote tracking | — | **Missing.** No remote registry or URLs of any kind — see [File-based distribution](#file-based-distribution-instead-of-remotes) below. |
| N/A — no counterpart | [`prikk doctor`](../guide/troubleshooting.md) | Health diagnostics with two real repairs (`--repair-wal-tail`, `--repair-index`); `--repair-main-ref` is recognized and always refused — there is no implemented repair behind it. |
| N/A — no counterpart | [`prikk format upgrade`](release-compatibility.md) | Moves a format-6 repository to format 7 in place, after the same verification `prikk verify` runs. Git's own `extensions.*` config changes are the nearest idea, but prikk's upgrade is one explicit verb, verified first, with no downgrade. |
| N/A — no counterpart | [`prikk unlock`](../guide/backup-restore.md) | Lists or clears a held file lock. Nothing in Git needs this because Git has no equivalent lock. |
| N/A — no counterpart | [`prikk trust maintainer add/remove`](../guide/security-setup.md) | Manages the local MAINTAINER trust set. Git has no signer-trust concept at this layer; the closest analogue is a GPG keyring, and it is not the same model. |

## Commit versus seal, a two-phase model

Git's `commit` does one thing: it creates a commit object and moves the branch pointer in the same
step. prikk splits that into two commands with nothing in between them optional:

```sh
prikk commit --from-worktree -m "message"   # queues a signed Patch to the local WAL
prikk seal --allow-no-audit                 # publishes the queue as a Block, moves the ref
```

A queued-but-unsealed commit is genuinely not part of history yet — it does not appear in `log`, and
a peer reading your repository cannot see it. You can queue several commits before sealing them
together. There is no Git operation that maps onto `commit` alone; it is closer to `git add` plus
`git commit --no-verify` into a holding area that Git does not have, and `seal` is the step that
actually behaves like `git commit`'s publishing half.

## A current branch, no `HEAD`

There is no `HEAD` and no detached state, but there is a current branch: `.prikk/current-branch`
names one local branch (`heads/main` in a new repository), and every command that takes `--ref`
defaults to it. `prikk branch switch heads/<name>` moves the worktree and the pointer together. Unlike
`git switch`, it never carries uncommitted changes across: a dirty worktree, or unsealed work queued
for another branch, is refused, and you commit or seal first. `--ref` given explicitly still works on
every command without switching.

## Messages are stored; authors and dates are not

`prikk commit -m <message>` requires a non-empty message and **stores it** — since `0.32.0` it is a
signed, identity-bearing field on the `Patch` itself (`Patch` schema 4), and `prikk log` prints it
under its block, one line per patch. Changing a message changes the patch id, exactly as changing an
operation does: it is evidence, not an annotation
([RFC 123](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/123-commit-message-and-authorship-metadata.md)).

**A patch written before `0.32.0` carries no message and shows no message line** — absence, not a
placeholder. Those patches stay readable forever; the message was never recorded and cannot be
attached retroactively.

**An author display name and a commit date are still not stored, and the date never will be.**
`created_at` is pinned to zero at every signing site so object ids stay reproducible across machines,
which CI proves by mutating a repository on Windows and Linux and diffing the resulting id lists. A
timestamp inside the identity surface would destroy that property. The display name is a separate,
deferred question (RFC 123 §5). A `tag`'s message is persisted too — the two commands are now
symmetric in this respect.

## File-based distribution instead of remotes

There is no remote registry, no URLs, and no network transport of any kind in the binary. Moving
history between repositories is always a file you produce and hand to the other side yourself:

- [`prikk bundle export`/`import`/`verify`](../guide/backup-restore.md) — a complete, self-contained
  history bundle in one file.
- [`prikk sync`](../guide/sync.md)'s `summary`/`compare`/`have`/`build`/`accept`/`pending`/`seal`
  subcommands — an incremental, gap-closing exchange, still file-based, still no sockets.

Either way, what you receive lands as an untrusted `remotes/<name>` pointer — readable by `log`,
`merge-evidence`, and `merge`, but not automatically merged or trusted the way a Git remote-tracking
branch is once fetched.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| `commit` only queues a signed Patch to the local WAL; `seal` publishes it as a Block and moves the branch ref. | [`node_authoring.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_patch/node_authoring.rs), [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs) |
| There is no `HEAD`; `--ref` defaults to the branch `.prikk/current-branch` names, and `branch switch` refuses a dirty worktree or unsealed work for another branch. | [`current_branch.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/current_branch.rs), [`branch_switch.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/branch_switch.rs), [`branch.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/branch.rs), [`commands.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/commands.rs) |
| `commit -m`'s message is stored as an identity-bearing `Patch` field since `0.32.0`; `tag create -m`'s message is persisted too. | [`main.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/main.rs), [`payload/tag.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/payload/tag.rs), [RFC 123](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/123-commit-message-and-authorship-metadata.md) |
| There is no staging area; `commit --from-worktree` always considers the whole worktree, filtered only by `.prikkignore`. | [`worktree_files.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_patch/node_authoring/worktree_files.rs), [`ignore.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/ignore.rs) |
| No remote registry or network transport exists; distribution is `bundle export`/`import`/`verify` or `sync`, both file-based, landing as an untrusted `remotes/<name>` pointer. | [`bundle.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/bundle.rs), [`sync.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/sync.rs), [`received.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/received.rs) |
| Every command named on this page is a real registered command, and every registered command is named on this page. | [`commands.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/commands.rs) (`COMMANDS`), checked mechanically by [rules (A) and (C)](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/commands/tests.rs) |

## Provenance

Seeded from the external architecture audit's 2026-08-31 feature-completeness matrix
(`audit-2026-08-31-task-1a-design-functionality.md` §4), re-verified against `main` at the time of
writing rather than copied — several rows in that matrix are now stale (`worktree-status` was broken
then and is fixed; `.prikkignore` did not exist then and does now). [RFC
128](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/128-outward-facing-project-surface.md)
§5 requested this page. It does not cover RFC 113 (history import) or any importer tool — neither
exists yet.
