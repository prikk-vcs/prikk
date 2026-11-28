# RFC 174 — What a JSON consumer may rely on

**Status.** **ACCEPTED 2026-11-28 by the owner** (*"Accepted. Just in case, verify carefully they match my fundamental philosophy."*).
Proposed 2026-11-27 by the architect, from the 0.59.0 J design round (J1 to J2b, `rfcs/handoffs/173-the-json-tier/`).
- **The architect's reading:**
  - the design is accepted, with §6's build order as the recommended order;
  - **how much of it 0.60.0 carries is set in the 0.60.0 scope,** which the owner authorizes;
  - the verification asked for found five places, corrected before acceptance was recorded:
    - the `unlock` path form (§4.4);
    - `repair.argv` safety (§4.1);
    - `publication_trust_issues[]` keeps "every item blocks" (§4.1);
    - the exit-code promise and signals (§3.6);
    - `log --paths` on a received ref names what works (§4.7), with the page listing where `refused` can appear (§3.2).
- **Nothing here is built.** J's builds are **0.60.0 at the earliest.**
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
   - **`refused`, the fallback for an unclassified failure.** **The page lists the commands that can print it** (from the conformance
     test's cases), so it never surprises a consumer, and a release that classifies one names the new id in `### Output changes`.
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
   - **No other code from prikk.** A process ended by a signal, or by a wrapper's timeout, is outside the promise, and the page says so.
7. **The consumer rows:**
   - **M1a:** planeter's `received[].closed`/`current` never existed. Documented, and told.
   - **M1b:** `received[].signed_by` is added: a P array of key ids, empty if the tip carries no signature.
   - **M2:** `show`'s path union (`{path}` or `{unresolved_node_id, reason}`) is a documented A rule.
   - **M3:** stikk's exit-2 premise was a malformed-versus-absent confusion. Told.

## 4. New fields and forms

1. **`verify-report-v1`, additive:**
   - `ref_publication_issues[]`: code, ref name, message, blocking. **An in-flight publication is `blocking: false`, and `verdict.ok`
     stays true.**
   - `message` on `publication_trust_issues[]`. **That array keeps its meaning: every item blocks** (`verdict.ok` is false while it holds
     one). A finding that does not block never goes into it, but into its own array: for example RFC 172 §3D's `received_unadopted[]`, if
     that is built;
   - `ways_out[]`: kind, prose and **`repair`**, an N object with `action` ∈ {`command`, `sound-copy`, `wait`, `manual`} and an `argv`
     for `command`. It comes from the typed `WayOut`, tested against the commands the step-1e test types.
     - **`argv` is prikk's arguments only:** never a program name, a path to an executable, or a shell string. Every value in it that
       comes from the repository (a ref name, a block id) is one prikk has already validated as such.
     - **prikk never runs it.**
     - **`plan_first: true` means run it with `--plan-only` first.** A consumer that runs `repair` without a person must honour that, and
       the page says so.
     - The same rules hold for `doctor`'s `repair` (§4.3).
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
     closed** (planeter decides on it). *Superseded by Amendment 2: planeter reads no such code, so `code` is open with no exception.* **Whether inklkunn needs notice for other non-`info` codes is asked in its letter;**
   - `summary` counts that add up.
   - **`repair` is non-null for every issue whose recommendation names a command a user can type,** enforced by a test against the
     recommendation text. It is null only for genuinely manual advice.
   - Plain `doctor` only; the repair modes stay text.
4. **`unlock-listing-v1`:**
   - `locks[]`: a **repository-relative path that starts with `.prikk/`** (for example `.prikk/refs/locks/<…>.lock`), **kind (closed:
     inklkunn stops on an unknown lock kind)**, the recorded pid and start (N).
     - **`unlock --lock` is unchanged:** it takes a path as the operating system reads it. The page says to join the listed path with the
       repository's path.
     - *Changed at the verification (2026-11-28):* the design had `unlock --lock` accept a `.prikk`-relative form. A relative path given to
       `unlock --lock` today means relative to the working directory, so one text would name two locks, and a script could clear the wrong
       one. The `.prikk/` prefix reads one way only, and needs no parser change;
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
   - Refused on a received ref, **naming what works there** (`prikk show <block>` per block).
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

## 7. Amendments: the consumers' answers

### Amendment 1 (the architect, 2026-11-28): stikk and inklkunn

From stikk 025 and inklkunn's letter of 2026-11-28 (planeter's is Amendment 2). Both are additive and within §2's rules.
**The owner is told, and may refuse either.**

1. **The closed list is confirmed** by stikk and inklkunn for every set they read.
   - **`doctor`'s `code` stays open** (inklkunn: the page listing every code, with a new one named in `### Output changes`, is enough).
     The `PRIKK-DOCTOR-UNREACHED-*` family stays closed for planeter.
   - **`merge-plan-v1`'s `outcome` is confirmed closed** (stikk decides on it).
   - **Shipped spellings a consumer reads,** named so no one renames them:
     - `log`'s `blocks[].kind` **`Root`, `Normal` and `Merge`** (the J1 runs saw only the first two; the code has three, and stikk reads
       `Merge`);
     - `status`'s queued `operations[].kind` `rename-path`;
     - `trust maintainer add`'s key-id conflict ids.
2. **`merge-plan-v1` gains `action_message`** (P, prose, not promised in its wording), beside the closed `action` id. stikk shows
   prikk's own words for the next step and never writes its own paraphrase.
3. **`log --after <block-id>`** (inklkunn): lists the blocks after the named one, up to the ref's tip, so an index that keeps its head adds
   one increment at a time.
   - **With `--paths`, the walk starts from the named block's state** (its nearest checkpoint, with the 0.60.0 `show` fix), not from the
     root.
   - **Refusals:**
     - a block the repository lacks: `point-not-found`;
     - a block that is not in the ref's history: `not-an-ancestor` (new), whose message names rebuilding the index from the root,
       `prikk log --ref <ref> --paths`.
   - Without `--after`, `log` is unchanged.
4. **`refusal-v1` on `verify` and `worktree-status`:**
   - inklkunn reads it now;
   - stikk will read every `refusal-v1` before its own schema check in its next re-baseline, and will say when that has shipped.
   **The notice is given.**
5. **`bundle-verify-v1`'s sentence** that it verifies structure and no signature is kept on the page. stikk's screen will say the same.

### Amendment 2 (the architect, 2026-11-28): planeter's answers

From planeter's letter of 2026-11-28. All of it is additive or a narrowing, within §2's rules. **The owner is told, and may refuse any
item.**

1. **The closed list is confirmed by all three consumers.**
   - **`doctor`'s `code` is open with no exception.** The `PRIKK-DOCTOR-UNREACHED-*` family was made closed because planeter was thought
     to decide on it. planeter reads no such code, so the exception goes (it supersedes Amendment 1's sentence). Every new code is named in
     `### Output changes`, as §4.3 already says.
2. **`sync-accept-v1`: the page says in words a reader new to the store can follow:**
   - **`written_object_count`:** objects this run wrote that the repository did not hold before;
   - **`merged_object_count`:** objects the repository already held, to which the artifact added a signature. No new object was written
     for them. The text form already says *"objects gaining signatures"*.
   - **A refusal is `refusal-v1` and carries no count.** Whether the store changed is the refusal id's own row (§3.4). **`lock-conflict`
     gets that row for `sync accept`, pinned by a test,** since planeter reads it after a lock conflict.
3. **`doctor`'s `repair` for planeter's four repairs:** yes.
   - **The shape:** `{"action": "command", "argv": ["doctor", "--repair-index"], …}`, likewise for `--repair-wal-tail`,
     `--repair-pointer-index-tail` and `--repair-tails`. Plain `doctor`'s issues recommend all four today (`crates/prikk-store/src/doctor.rs`).
   - **`plan_first` is true only for `--repair-tails`,** the one with a `--plan-only`.
   - **`argv` names no repository:** it acts on the repository the report was read from, and the page says how to pass that path.
   - **`repair` is typed at each site, never parsed from the prose.** `doctor`'s recommendations are prose strings today, so the build adds
     a typed repair beside each one. **The test checks agreement both ways:**
     - every `argv` appears in its prose;
     - every prose that tells a user to run a command has one.
     A sentence that names a command only to say it does not help is not a command repair. Two exist today, in `crates/prikk-store/src/way_out.rs`: one for interior damage and one for a missing RefState.
   - *"The repair modes stay text"* (§4.3) means **the output of `doctor --repair-…` itself,** not plain `doctor`'s `repair` field.
4. **Three more `details` objects.** Each value is already at the site (read 2026-11-28):

   | id | `details` | presence | where the value comes from |
   |---|---|---|---|
   | `lock-conflict` | `{holder_pid}` | **A:** only when the lock judge read a pid | `crates/prikk-store/src/lock.rs` |
   | `over-max-bytes` | `{size, max_bytes}` | P, both | `crates/prikk-store/src/point_reading.rs` |
   | `interrupted-publication` | `{ref_name}` | P | `crates/prikk-store/src/ref_completion.rs` |

   - **`holder_pid` is the pid the holder recorded.** It can name another process in another pid namespace. The page says it is for a
     person to read and never something to decide on, which is how planeter uses it.
   - **`size`, not planeter's `size_bytes`:** `tree`, `show` and `checkout --patch-plan` already call a file's byte count `size`, so one
     meaning keeps one name. `max_bytes` is the flag's own name, and `ref_name` has its shipped meaning.
5. **§3.5's notice is accepted by all three:**
   - planeter reads `schema_version` first, so a `refusal-v1` from `verify` becomes a typed refusal for them, not an empty stdout;
   - **`log`'s and `worktree-status`'s move to exit 2 is neutral for planeter,** who will run its suite on the release that makes it.

**Noted, not ours to change:**
- planeter matches the substring `lock conflict` in prikk's stderr today (`planeter-transport/src/propose.rs`). The prose is not
  promised; **once §3's seam ships, the `lock-conflict` id is,** and the acknowledgement letter says so.
- planeter builds nothing on this RFC until a release names it in `### Output changes`.

## 8. Decided

1. **Accepted by the owner, 2026-11-28,** with the verification's five corrections folded in (Status).
2. **The build order** is §6's. **How much of it 0.60.0 carries is the 0.60.0 scope's.**
