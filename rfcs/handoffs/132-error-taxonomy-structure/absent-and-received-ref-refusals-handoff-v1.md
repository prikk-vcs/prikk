# Absent refs, received refs and missing repositories — refusal sweep handoff v1

**Order of work for 0.45.0 (the four handoffs of 2026-09-17):** 1. `136-block-aggregation-payoff/warm-cache-commit-anomaly-handoff-v1.md` (measure-only); 2. `135-first-run-entrance-and-configuration/distinct-default-key-ids-handoff-v1.md`; 3. `144-two-point-comparison/merge-with-renames-design-handoff-v1.md` (report only); 4. `132-error-taxonomy-structure/absent-and-received-ref-refusals-handoff-v1.md`. Take them one at a time, each reported and reviewed before the next.

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

## Addendum 1 2026-09-17 — order

This round starts **after** the merge-with-renames implementation
(`144-two-point-comparison/merge-with-renames-design-handoff-v1.md`, Addendum 1) has been reviewed. Both change
`merge`: this round its ref resolution (`merge/execute.rs:153`, `merge/evidence.rs:118`), that round its confluence
algebra. Measure §1 again on the binary at that point before implementing.

## Addendum 2 2026-09-17 — scope ruled; implement

The stop was right (review `absent-and-received-ref-refusals-stop-review-v1`). Rule 1 read as "every ref-taking
command" would break two documented flows: `sync have <ref>` on a ref the receiver lacks is the first step of a sync
(`guide/sync.md:30`), and `bundle preview` answers `no-local-history` by design. The inventory also found three more
consumers of received refs that work today.

**Rulings on §4:**
1. **Rule 1 applies to consumers only.** A consumer is a command whose operation reads an **existing** ref's state.
   The list is the report's §4.1 list, **including `checkout --plan-only`**:
   - `log`, `worktree-status`;
   - `checkout` (all modes, including `--plan-only`), `inverse-plan`, `rollback-preview`, `rollback-draft`,
     `rollback-draft-verify`;
   - `merge-evidence`, `merge-plan`, `merge --into/--from`;
   - `bundle export`, `branch create --from`, `tag create --target`, `branch close`, `branch switch`, `sync build`.

   **Not consumers, unchanged:**
   - `sync have` and `bundle preview`, which answer "none of it" as a state;
   - the creators: `commit --ref`, `seal --ref`, and `branch create`'s own name.

   `branch switch` already refuses with `Precondition` and names `branch create` as its route. It keeps its own route
   sentence, and the control runs that route.
2. **Existence before anything else, in every consumer.** `rollback-draft-verify` and `sync build` resolve the ref
   before the WAL check and the have-list mismatch check.
3. **Rule 2 applies where today's answer is false:**
   - `checkout` (every mode), `inverse-plan`, `rollback-preview`;
   - `bundle export` ("does not exist");
   - `branch create --from`, `tag create --target` ("does not resolve").

   The `invalid name: ref namespace is reserved` refusals stay as they are. They come from name validation
   (`refs.rs:595`, `:725`): the name is refused as a target of that operation, which `InvalidName` states truly. Rule 7
   holds. Their wording for readers (`worktree-status`, `sync have/build`) is recorded as a candidate, not changed
   here.
4. **Routes.** A refusal names a route only if the control can run it **from that refusing state in general**.
   - For a received ref given to `checkout`, `inverse-plan`, `rollback-preview`, `bundle export`, `branch create
     --from` or `tag create --target`: **name no route.** Say that received refs are read by `log`, `merge-evidence`,
     `merge-plan` and `bundle preview`, and taken into a local branch by `merge --from`. That sentence is a factual
     list, and the control runs `log --ref` and `merge-plan` on it.
   - `branch create --from remotes/…` does not work today, so it is not named. `bundle export` of a received ref may
     add "a whole repository will travel in one file under RFC 155" only as future tense in docs, never in the
     refusal.

**Rule 5 proceeds as written.** Include `log`, `doctor` and `worktree-status` among control 5's commands.

**Control 6 grows** to the five received-ref consumers that work today (`log`, `merge --from`, `merge-evidence`,
`merge-plan`, `bundle preview`). Add one control holding the non-consumers: `sync have <absent>` still writes an empty
have-list with exit 0, and `bundle preview --ref <absent>` still reports `no-local-history`.

**Report:** `.git-exclude/review-request/absent-and-received-ref-refusals-report-v2.md`.

## Addendum 3 2026-09-17 — accepted; one follow-up

**Accepted** (review `absent-and-received-ref-refusals-review-v1`): `bbdbcb71`.
- **Deviation 1 is confirmed.** `sync build` is not a consumer. RFC 116 §4 makes a ref the sender lacks "already in
  sync", the same "none of it" state as `sync have`. Ruling 1 listed it in error, and ruling 2 does not apply to it.
- **Finding 2** (`rollback-draft-verify --ref remotes/…` answers with its WAL check) is recorded as a candidate.

**Follow-up: the implicit current branch in `checkout`.** The gate in `run_checkout` also runs with no `--ref`. In a
fresh repository, `checkout --plan-only` used to exit 0 with `ref-state: <not published>` and the note "publish a ref
before checkout can target a block". It now refuses. This contradicts ruling 4's principle (the implicit current
branch of a fresh repository is a legitimate state) and the CHANGELOG's own sentence ("answers exactly as before").
Rule:
- **`checkout --plan-only` without `--ref`** keeps its previous answer, byte-for-byte, exit code included.
- **Every other checkout mode without `--ref`** keeps the new rule-1 wording. It was already a refusal (`integrity
  error: … is not published`), so this is the intended class fix; name it in the CHANGELOG.
- **With `--ref`**, nothing changes from this round.
- **Control:** in a fresh repository, `checkout --plan-only` (no `--ref`) is byte-identical to the pre-sweep binary
  (`63d0dcee`), and `checkout --plan-only --ref heads/nope` refuses. *Perturbation:* gate the implicit ref.

**Report:** `.git-exclude/review-request/absent-and-received-ref-refusals-follow-up-report-v1.md`.

## Addendum 4 2026-09-17 — URGENT: `main` is red on macOS and Windows

**CI on `cb1951d5` (run 35175704741) failed two jobs.** Both failures are in this round's tests, not in the product.
- **macOS mutation test suite:** `control4_…` and `control5_…` fail. The temp directory `/var/folders/…` is a symlink
  to `/private/var/…`, and `prikk` reports the resolved `/private/var/…` path. The tests build the expected text from
  the unresolved path.
- **Windows mutation test suite:** `control4_…` fails. The expected `log --format json` is a hand-built string holding
  the raw path. The real JSON escapes each backslash, so `C:\Users…` appears as `C:\\Users…`.

**Fix before anything else:**
1. **Never compare an absolute path as text.** The product prints different path forms per platform: the resolved
   `/private/var/…` on macOS, but the unresolved `C:\Users\RUNNER~1\…` on Windows. There, `std::fs::canonicalize`
   gives yet another form, `\\?\C:\Users\…`. So "canonicalize the expected path" alone would fix macOS and break
   Windows. Instead:
   - **split the output at the path**, and compare the path part by *identity*: `std::fs::canonicalize` applied to
     **both** the printed path and the fixture path, then compared;
   - compare the rest byte for byte;
   - for JSON, **parse** the output, compare every non-path field, and compare the path field by identity. Never
     format expected JSON by hand.

   Use one shared helper for this in `tests/support`, so the next round inherits it.
2. **Check `63f4fc41`'s control** (`addendum3_implicit_plan_only_keeps_its_answer_in_a_fresh_repository`), which
   embeds a path into a byte-for-byte expectation. It has the same exposure: compare the path by identity, per item 1.
3. **Sweep this round's test file** for every other expectation built from a path.
4. **Local gates cannot catch this** (Linux only, and cross-target clippy runs no tests). The report must say which
   platforms each fixed assertion was reasoned for, and the architect reads the Windows and macOS jobs after the push.

One commit on top of `63f4fc41`. **Report:** `.git-exclude/review-request/absent-and-received-ref-refusals-ci-fix-report-v1.md`.
The follow-up `63f4fc41` stays unpushed until this fix lands with it.
