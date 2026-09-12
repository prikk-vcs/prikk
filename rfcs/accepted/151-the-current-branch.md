# RFC 151 — The current branch, and `branch switch`

**Status.** **ACCEPTED 2026-09-13.** The owner's word on the review that carried this RFC's three questions was *"Will be handed to dev team"*; **the architect reads that as the three recommendations of §3 taken as ruled (yes, yes, yes)** and records the reading here so it can be corrected before increment 2 makes it irreversible. Handoff: `151-the-current-branch/current-branch-handoff-v1.md`. Originally: **PROPOSED 2026-09-13 by the architect**, on the owner's instruction to review the open
directions. The gap it closes is recorded in three places as *"the largest usability gap in the
product"* (`ROADMAP.md`), deferred by DC-61 *"until after the queuing decision"* (DC-66, landed), and
named in `prikk --help` as *"a separate, not-yet-designed increment."* This is that design. **It changes
a deliberate stance** — every command resolves `--ref` explicitly — so the owner accepts the direction
first; nothing is handed off until then.

## 1. What exists today

- Every command that takes `--ref` defaults it to the constant `heads/main` (`args.rs`,
  `DEFAULT_CHECKOUT_REF`). There is no state that says which branch a worktree is on; there is the
  constant.
- The active WAL (`DEFAULT_ACTIVE_NAME`) is **owned by a ref** once non-empty (`ActiveRefOwnership`):
  `commit --ref X` binds unsealed work to `X`; `seal` seals into it; `branch close` reads the ownership
  and refuses to close a ref that owns unsealed work. So half of a current branch already exists: the
  ref your unsealed work belongs to.
- The materializers are additive and safe by design: they refuse conflicting files and **never remove
  files** (`worktree.rs`, `patch_checkout.rs`). A switch that changes the baseline without changing the
  worktree would make the next `commit --from-worktree` author the difference between two branches as a
  patch — the reason DC-61 deferred this rather than shipping a pointer.

## 2. The design

### 2.1 The pointer

`.prikk/current-branch`: one line, a local branch ref name (`heads/<name>`). **Local, mutable, unsigned,
and never trusted for anything but a default**: not read by `verify`, not consulted by any trust or
signing decision, not carried by `bundle`, `sync` or any artifact, not part of any object. Read with
validation: it must name an existing local branch ref (or the unborn default of a fresh repository), or
the command refuses as `Precondition` naming the file. Written by `init`/`setup` (`heads/main`) and by
`branch switch` — nothing else. `doctor` reports a pointer that names a missing or closed ref.

### 2.2 The default

Wherever `--ref` defaults to `heads/main` today, it defaults to the pointer. `--ref` given explicitly
behaves exactly as now. A repository without the file (created before this RFC) reads as `heads/main`,
so nothing changes for anyone until they switch.

### 2.3 `prikk branch switch heads/<name>`

Refuses, in this order, each a `Precondition` with the route named:
1. the target is not an existing local branch ref (`branch create` first) or is closed;
2. the active WAL holds unsealed work owned by another ref (`seal` or roll back first);
3. the worktree is not clean against the current branch's baseline (`worktree-status` names the paths;
   commit or revert first).
Then: materialize the target's baseline over the clean worktree — create and modify as today's
materializer does, and **delete a tracked file only when its content is identical to the current
baseline** (the clean-worktree precondition makes that every tracked file the target lacks); untracked
files are never touched. Then write the pointer. The transition is one operation with the same crash
posture as `checkout --patch-materialize`: interrupted, the pointer is not yet written and the worktree
is a superset of both trees, never a torn file.

### 2.4 Visibility

`worktree-status` and `log` name the branch they resolved; `branch list` marks the current one;
`prikk --help`'s note is replaced by `branch switch`'s synopsis. `--format json` carries
`"current_branch"` where the ref is already reported.

## 3. Decisions for the owner

1. **A local mutable pointer as the default for `--ref`** — yes or no. (Architect: yes; the explicit
   form stays; the pointer is a default, not an authority.)
2. **`switch` requires a clean worktree and no unsealed work for another ref** — yes or no.
   (Architect: yes. A switch over dirty state is where every VCS's data-loss stories live; refusing is
   the "clean over rich" answer, and `worktree-status` already names what is dirty.)
3. **Deletion by the switch**, bounded as in §2.3 — yes or no. (Architect: yes; without it a switch is
   not a switch. The bound — identical to the current baseline — is what keeps the materializers'
   posture.)

## 4. Non-goals

No branch rename or delete (DC-61 closed branches instead). No remote branches. No detached state — the
pointer always names a branch. No change to trust, signing, sync, or any format.
