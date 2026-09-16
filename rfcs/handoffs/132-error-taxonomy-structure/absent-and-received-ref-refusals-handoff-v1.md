# Absent refs, received refs and missing repositories — refusal sweep handoff v1

**Live 2026-09-17.** 0.45.0 item 3, the refusal-class sweep (ROADMAP schedule row, and §C "ref is not published"
answers three classes). This is one cross-command sweep in the RFC 132 mould, not a per-command fix.

## 1. Measured 2026-09-17, binary of `fab4aded`

Setup:
- **`src`**: a set-up repository with one sealed block on `heads/main`.
- **`dst`**: a fresh `init` that imported `src`'s bundle, so it holds `remotes/heads/main` and no local branch.
- **`heads/nope`** exists in neither.

| Command | What it says | Exit | Problem |
|---|---|---:|---|
| `checkout --patch-plan --ref heads/nope` | `integrity error: ref heads/nope is not published` | 1 | an absent name is not damage |
| `rollback-preview --ref heads/nope` | `integrity error: … is not published` | 1 | same |
| `rollback-draft --append-inverse --ref heads/nope` | `invalid name: … is not published` | 1 | a third class for the same fact |
| `merge-plan … --left-ref heads/nope`, `merge --into heads/nope`, `merge --from heads/nope` | `integrity error: … is not published` | 1 | as above |
| `bundle export --ref heads/nope` | `integrity error: ref heads/nope does not exist, nothing to export` | 1 | class |
| `branch create --from heads/nope`, `tag create --target heads/nope` | `--from ref heads/nope does not resolve to a published ref` | 1 | a fourth wording |
| **`checkout --snapshot-plan --ref heads/nope`** (also `--snapshot-materialize`) | `precondition not met: checkout target for heads/nope is not a checkpoint … use prikk checkout --patch-plan --ref heads/nope` | 1 | **false, and the named route refuses** |
| **`log --ref heads/nope`** | `history: <empty>` (JSON: an empty listing) | **0** | a typo reads as an empty history |
| **`worktree-status --ref heads/nope`** | `worktree has changes against the baseline` | 1 | **false** |
| in `dst`: `checkout --snapshot-plan --ref remotes/heads/main` (also `--snapshot-materialize`) | `… is not a checkpoint, so it carries no snapshot` | 1 | false: the real reason is that a received ref cannot be checked out |
| in `dst`: `checkout --patch-plan --ref remotes/heads/main` | `integrity error: ref remotes/heads/main is not published` | 1 | it exists: it is *received* |
| in `dst`: `bundle export --ref remotes/heads/main` | `integrity error: … does not exist, nothing to export` | 1 | false: it exists, but a received ref cannot be exported (RFC 155's artifact is where that lands) |
| `verify <path with no repository>`, `format upgrade <same>` | `i/o error: No such file or directory (os error 2)` | 1 | names no path and no cause |

## 2. Ruling (architect, 2026-09-17)

1. **One resolver, one refusal, for a named ref that does not exist.**
   - Every command that takes a ref name (`--ref`, `--into`, `--from`, `--left-ref`, `--right-ref`, `--target`, and
     any other found by inventory) resolves it through **one shared function**.
   - That function refuses an absent name with `Precondition`: `ref <name> does not exist in this repository`.
   - Where `remotes/<name>` exists, it adds one sentence naming it. That sentence is a claim, so the control runs
     what it suggests.
   - Integrity stays for damage only (RFC 132).
2. **A received ref given to a command that cannot use one** refuses with `Precondition`, naming it as **received**
   and saying that this command does not accept received refs. Where a route exists that works today, name it and
   test it.
   - **Inventory first:** list every ref-taking command, and which accept `remotes/…` today. `log` does, and so does
     `merge --from` (DC-85). Nothing that works today may start refusing.
3. **Existence before kind.** `checkout --snapshot-plan` and `--snapshot-materialize` decide "absent" (rule 1) and
   "received" (rule 2) **before** "not a checkpoint". The checkpoint message is only for an existing, local,
   non-checkpoint target.
4. **An explicit absent ref never reads as a state.**
   - `log --ref <absent>` and `worktree-status --ref <absent>` refuse under rule 1, in prose and JSON, with exit 1.
   - The **implicit** current branch of a fresh repository (unpublished `heads/main`, no `--ref`) keeps today's
     answers. That is a legitimate state, and the control must hold both sides.
5. **A path with no repository** refuses with `Precondition`: `no prikk repository at <path>`, in every command that
   takes `[path]` or opens the repository from the working directory. It is decided in one place, at layout open.
   A real I/O failure inside an existing repository keeps `Io`.
6. **`branch create` and `tag create`** move onto rule 1's wording and class.
7. **No other class changes.** Anything else found wrong along the way is reported, not fixed.

## 3. Controls (each shown failing under its perturbation, then restored byte-identical)

1. **A table-driven CLI test over every ref-taking command × {absent, received}.** It asserts the class prefix, the
   exit code and the wording for each. *Perturb: give one command its own copy of the resolver (perturb the sharing).*
2. **Every route a refusal names is run** from the refusing state and succeeds (rules 1–2). *Perturb: name a route
   that refuses.*
3. **`checkout --snapshot-plan`**: an absent target gets rule 1; a received one gets rule 2; a local non-checkpoint
   still gets the checkpoint message. *Perturb: the checkpoint check first.*
4. **`log`, `worktree-status`:** an explicit absent ref refuses; a fresh repository with no `--ref` keeps today's
   output byte-for-byte. *Perturb: treat absent as empty.*
5. **A missing repository path** gets rule 5 for `verify`, `format upgrade`, `status` and at least two other
   commands. An I/O failure injected inside a real repository stays `Io`. *Perturb: map every `Io` to the new
   refusal.*
6. **Nothing that accepted a received ref starts refusing:** `log --ref remotes/…` and `merge --from remotes/…` keep
   passing. *Perturb: apply rule 2 to them.*

Run these on every CI platform. Do not Linux-gate unless a fixture needs it, and say why if one does.

## 4. Docs and CHANGELOG

- **CHANGELOG `### Changed — refusal classes for absent and received refs`:** list the prefix changes (`integrity
  error:` and `invalid name:` → `precondition not met:`) for consumers who match on text. Also the `log` and
  `worktree-status` exit change for an explicit absent ref.
- `commands.md` where a command documents its refusals.

## 5. Report

`.git-exclude/review-request/absent-and-received-ref-refusals-report-v1.md`, including the rule-2 inventory table.
**Stop and report before implementing** if the inventory shows a command where rule 2 would stop something that works
today.
