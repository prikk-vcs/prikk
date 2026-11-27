# RFC 174 — What a JSON consumer may rely on

**Status.** **PROPOSED 2026-11-27 by the architect,** from the 0.59.0 J design round (J1 to J2b, `rfcs/handoffs/173-the-json-tier/`).
- **Nothing here is built.** Per the owner's ruling of 2026-11-18, J's builds are **0.60.0 at the earliest, after the owner reads the
  round.**
- **The three consumers** (stikk, inklkunn, planeter) answered what they read on 2026-11-19 (stikk 024). The letters that send this
  design to them are drafted in J3 and held for the owner.

**Every part is held to the owner's philosophy:**
- no claim wider than the mechanism (a promise is only as wide as the test that keeps it);
- nothing that confuses a reader;
- less is more;
- no unactionable state.

**And to the consumers' standing constraints:**
- no negative or fractional number, and no nesting deeper than 16;
- **never switch a field between absent and `null`;**
- notice before a value is added to a closed set;
- `prikk --version` stays `prikk X.Y.Z`;
- prose stays on stderr beside `refusal-v1`;
- **never a `refusal-v1` with an exit code other than 1.**

## 1. What is true today (measured on the 0.59.0 candidate, J1)

- **22 commands or modes have `--format json`** (23 schema names). `sync accept`, `doctor`, `unlock`, `merge-plan` and `bundle verify`
  have none. `--format json` on a command with none is exit 2.
- **Numbers and nesting:** every number is an unsigned integer, and none above 2^53 is reachable. The deepest nesting is 8.
- **Refusals:** five commands print `refusal-v1` (`cat`, `extract`, `bundle import`/`preview`, `archive import`, `trust maintainer add`);
  **twelve print nothing on stdout.**
- **Exit codes:**
  - a **malformed** point exits 2 in six readers, but 1 in `log` and `worktree-status`;
  - a **well-formed absent** point exits 1 everywhere.
- **`verify --format json` has no ref-publication findings** (an in-flight publication is invisible in it), **no `ways_out`**, and no
  warning counters.
- **Of 220 field reads the consumers stated, 217 hold;** the three rows are M1 to M3 below.
- **Per-block paths:** `show` per block is the only way today. About 88% of its cost is a replay from the root (the 0.60.0 `show` fix),
  and **a 2.5 ms process floor per call stays after any fix.**

## 2. The promise

1. **One reference page, `docs/src/reference/json.md`:** one section per schema. Each field has:
   - its type;
   - **its presence class:** **P** always present, **N** present and nullable, or **A** absent by a stated rule (never moving between N
     and A);
   - for an enum-like field, whether its set is **closed** or **open**;
   - the release that added it.
2. **Closed sets are the consumers' list** (they decide on the value), confirmed by them in the letters. **A value is added to a closed set
   only after a letter names it, and in the release whose `### Output changes` names it.** A set is open only if no consumer decides on
   it.
3. **`-v1` changes only additively.** A successor schema would be announced with at least two minor releases of overlap. **No successor
   is proposed, and no mechanism for one is named.**
4. **Numbers are unsigned integers at most 2^53 − 1, and nesting is at most 12.** Promised by argument, checked on every fixture.
5. **Not promised:** message, detail and note prose; hunk text; key order and whitespace; array order except where the page states one;
   any text output.
6. **Shipped spellings stay as they are** (`Normal`/`Root`, `not_evaluated`, the `prepo-` schema names, `import_refusals[].id`).
   **New names:** schema `<thing>-v<N>`, fields `snake_case`, values and ids lower-case kebab.
7. **A conformance test keeps the page true:**
   - every emitted path is on the page with its class;
   - every P path is present;
   - the bound and the depth hold;
   - **every rule inferred from code is proved by a fixture or corrected.**

## 3. Refusals and exit codes

1. **Every refusal of a command with a JSON form prints `refusal-v1` on stdout, with exit 1** (the prose still on stderr).
   - **The seam:** one place in the CLI's run loop, with the kind carried on the error (M-b).
   - **It prints only when nothing has been written to stdout,** so there is never a second document after a report. **The test:** one
     document per run, in every case of every form.
2. **The ids:**
   - `not-a-repository`;
   - `point-not-found`;
   - `object-not-found`;
   - `path-not-found`;
   - `lock-conflict`;
   - `interrupted-publication`;
   - `damaged-repository`;
   - **`input-not-found` and `input-undecodable`** (two actions, two ids);
   - the existing artifact ids;
   - **`refused`, the fallback for an unclassified failure.**
3. **`details`, an N field on `refusal-v1`:** an object defined per id. The claim ids, and the parent block id for
   `claim-parent-not-sealed` (planeter's ask), come from payloads the store already has.
4. **"The store did not change"** is written **per id, only for an id with a test that pins it.** `refused`, `io-failure` and
   `damaged-repository` carry no such row.
5. **`verify` and `worktree-status`** (which report and exit 1) print `refusal-v1` for a real refusal too, **after the letters give
   notice.**
6. **Exit codes, the whole promise:**
   - **0:** the command ran, and the document is the answer (including `bundle preview` with `import_refusals[]`, and `checkout
     --patch-plan` with `not_found`);
   - **1:** a refusal, or a report whose verdict is "no" (the schema name tells which);
   - **2:** a usage error, with **nothing on stdout.** A malformed point or path is exit 2 in every reader (`log` and `worktree-status`
     move).
   - No other code.
7. **The consumer rows:**
   - **M1a:** planeter's `received[].closed`/`current` never existed. Documented, and told.
   - **M1b:** `received[].signed_by` is added: a P array of key ids, empty if the tip carries no signature.
   - **M2:** `show`'s path union (`{path}` or `{unresolved_node_id, reason}`) is a documented A rule.
   - **M3:** stikk's exit-2 premise was a malformed-versus-absent confusion. Told.

## 4. New fields and forms

1. **`verify-report-v1`, additive:**
   - `ref_publication_issues[]`: code, ref name, message, blocking. **An in-flight publication is `blocking: false`, and `verdict.ok`
     stays true.**
   - `message` on `publication_trust_issues[]`;
   - `ways_out[]`: kind, prose and **`repair`**, an N object with `action` ∈ {`command`, `sound-copy`, `wait`, `manual`} and an `argv`
     for `command`. It comes from the typed `WayOut`, tested against the commands the step-1e test types.
   - `warning_counts`: P integer fields;
   - non-zero `trailing_partial_bytes[]`;
   - **`unreached_withheld`, a P boolean, true exactly when the text withholds the unreached list because a writer holds the lock.** The
     list is unchanged; the page says how to read the pair.
   - **No severity in `verify`.** `blocking` and `verdict.ok` are its decisions.
2. **`sync-accept-v1`:** counts, claim/author/tag signatures (the `sync-pending-v1` signature object), written and merged object counts
   ("whether anything was written"), `claims_file` (N).
3. **`doctor-report-v1`:**
   - it **embeds a whole `verify-report-v1`** (N);
   - `issues[]`: code, severity (closed `info|warning|error`), prose, and **`repair`**;
   - **code is an open set, with every `PRIKK-*` literal on the page by test, except the `PRIKK-DOCTOR-UNREACHED-*` family, which is
     closed** (planeter decides on it). **Whether inklkunn needs notice for other non-`info` codes is asked in its letter;**
   - `summary` counts that add up.
   - **`repair` is non-null for every issue whose recommendation names a command a user can type,** enforced by a test against the
     recommendation text. It is null only for genuinely manual advice.
   - Plain `doctor` only; the repair modes stay text.
4. **`unlock-listing-v1`:**
   - `locks[]`: a repository-relative path (relative to `.prikk`; `unlock --lock` accepts the form), **kind (closed: inklkunn stops on
     an unknown lock kind)**, the recorded pid and start (N);
   - **`liveness` (closed, 3 values) and `liveness_reason` (closed, 12 values).**
   - Listing only.
5. **`merge-plan-v1`:** baseline, sides, status and outcome (closed, kebab, with a text-to-JSON mapping table), a closed `action` id,
   items. `merge-evidence` is not designed.
6. **`bundle-verify-v1`:** ref, ref-state, tip, counts, and `manifest` (N, `bundle-preview-v1`'s shape). **The page says in words that
   it verifies no signature.**
7. **`log --paths`, for inklkunn's per-block paths:**
   - each listed block gains `operations[]` (`patch_id`, `kind`, `paths[]` as in `show`, no content). It is **present iff `--paths`**,
     and `null` for a block with no single starting state. **Without the flag, `log`'s output is byte-identical.**
   - **One fold, shared with `show`, and a test that it equals `show` per block.**
   - Refused on a received ref.
   - The page states that `--paths` costs a replay. About 65 to 75 ms for 2,000 blocks, bounded from measured parts: one call instead of
     1,999 (41 s today; ≥ 5 s even after the `show` fix).
8. **`log-report-v1` gains `truncated`:** a P boolean, true iff the ref holds more blocks than were listed (planeter ask 5).

## 5. What each consumer gets

| | stikk | inklkunn | planeter |
|---|---|---|---|
| the page, the closed list, exit rule | written down; confirm the list | the same; its text reads named not-promised | the same |
| `refusal-v1` everywhere | its last prose-refusal parses go; a worse message on `verify`/`worktree-status` until it reads the id | the same, fails closed until fixed | ask 3, with `details` |
| `verify` additions | additive keys | finding lines as JSON; `unreached_withheld` | **question 4 answered** |
| new forms | `merge-plan`, `bundle verify` (023) | `doctor`, `unlock` | `sync accept`, `doctor`, `unlock` (asks 1, 2) |
| `log --paths`, `truncated` | — | **its per-block index, linear** | ask 5 |

## 6. Build order (recommended; scheduling is the owner's)

1. **The seam, `refusal-v1` everywhere, the exit rule, the page and the conformance test.** Everything else rests on these.
2. **`verify`'s additions, `log --paths` and `truncated`.** The cheapest for consumers, and they answer two direct questions.
3. **`sync accept`, `doctor`, `unlock`** (planeter, inklkunn).
4. **`merge-plan`, `bundle verify`** (stikk).

The 0.60.0 `show` fix (ROADMAP row) is built with step 2. It gives `log --paths` its bounded start state.

## 7. For the owner

1. **Read and accept, amend or refuse.** Nothing is built before that.
2. **The build order** (§6), and how much of it 0.60.0 carries.
