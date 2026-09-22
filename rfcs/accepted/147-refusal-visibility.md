# RFC 147 — What the read surface promises about what a write will refuse

**Status.** **ACCEPTED by the project owner 2026-09-12**, with both cases as ruled in §2e and §3b. Opened 2026-09-10; both handoffs are live. Opened on two reports from the stikk project (letter 006 §4, §6),
**both reproduced here against the shipped 0.38.0 binary** before this RFC was opened.

**Author-review independence gap:** the architect authored this and will review its implementation.

---

## 1. The shape both reports share

A consumer that drives prikk decides what to offer a user from prikk's **read** surfaces —
`worktree-status`, `checkout --plan-only` — and then issues a **write**. **When a read surface describes
as ordinary something a write will refuse, the consumer offers an action that cannot succeed.**

stikk states the posture plainly, and it is the one this project agreed with on the cross-ref case:
*"we prevent refusals client-side rather than classifying them after the fact."* They already prevent an
empty commit and a wrong-ref commit **because `status` tells them**. The two cases below are the ones
the surface does not tell them about.

## 2. Case A — a symlink is `untracked`, and `unsupported paths` is `0`

Reproduced in **both** surfaces:

```
untracked files: 1
unsupported paths: 0
  untracked link.txt — worktree file is not in the baseline

{"path": "link.txt", "kind": "untracked", "detail": "worktree file is not in the baseline"}
```

Then:

```
$ prikk commit --from-worktree -m x
error: integrity error: worktree authoring: unsupported symlink authoring: link.txt:
       worktree symlink authoring is out of scope
```

**One such file blocks every commit in the repository**, and the machine-branchable `kind` says
`untracked`.

### 2a. Why it happens, at source

`worktree_status.rs:265-283`: **`UnsupportedPath` is produced in exactly one place** — the `Err` arm of
`RepoPath::parse`, i.e. a path whose **text** is not representable as a safe prikk path. A symlink has a
perfectly valid name, takes the `Ok` arm, and is classified `Untracked`. The `is_symlink()` check at
`:260` only stops *descent into* a symlinked directory; it never classifies the link. `:153-155` already
records this as a known blind spot.

### 2b. The design question, which is why this is an RFC and not a patch

**`unsupported-path` currently means "unrepresentable name". stikk needs "a write will refuse this".**
These are different predicates that happen to overlap.

**Widening the existing `kind` string to cover both silently changes what a machine-branchable value
means for every consumer** — precisely what RFC 140 §7b and RFC 142 §6b exist to prevent. So the options
are not equal:

1. **Widen `unsupported-path`.** Cheapest, and it redefines a shipped value. A consumer branching on it
   today gets new members without notice.
2. **A new `kind`** — e.g. `refused-by-authoring` — leaving `unsupported-path` meaning what it has always
   meant. Additive, and consumers ignore unknown kinds by construction.
3. **A separate report field**, not a `kind` at all, since "will this commit succeed" is a question about
   the *commit*, not about one path's classification.
4. **`commit --dry-run`**, which stikk names as a possible shape and does not ask for.

**Unruled. §6 records the architect's lean; the choice is the project owner's.**

### 2c. The error class is a second, separable defect

`commit` refuses with **`Integrity`** for what is a precondition — an unsupported input, not damaged
data. **This is the second such misclassification found on 2026-09-10**: `seal` reports an unadopted
maintainer key as `integrity error: publication trust policy is missing or unreadable`, when nothing is
missing or unreadable.

**RULED: neither fires an RFC 132 re-open trigger.** Those three (RFC 132 §5) are a first external
embedder, the CLI ceasing to flatten errors to strings, and a move toward library stability. **Choosing
the wrong existing variant is a defect against the taxonomy that already exists, not evidence that it
needs more variants.** Both are fixable today with nothing reopened.

### 2d. LOCATED and DELIVERED 2026-09-12 (`97f19b8d`) — three trust preconditions reclassified

The RFC 135 docs round (`6fac3575`) located §2c's two sites precisely, and verifying it produced a
third. All three opened at source.

| site | was | what actually happened | now |
|---|---|---|---|
| `trust.rs:214-217` `load_maintainer_trust_policy` | `Integrity("publication trust policy is missing or unreadable")` | `Ok(None)`: **nothing adopted yet**. The `?` already propagates real read errors; the damaged-snapshot case has its own message at `trust_index.rs:533` | `Precondition` |
| `trust.rs` `add_trusted_maintainer`, collision arm | `InvalidSignature("… already adopted with a different public key")` | a key-id collision detected **before any verification**; the adjacent arm makes re-adding the same key idempotent | `Precondition` |
| `seal`, signer id absent from policy | `InvalidSignature("maintainer signer key id X is not trusted by policy")` | policy **membership**, not a signature check | `Precondition` |

**One nuance that makes the first a wording decision, not a variant swap**: `Ok(None)` also arises from
a container truncated below its own header, which replays as no entries. The message therefore leads
with the overwhelmingly common cause and the actionable step (`prikk trust maintainer add`) **without
claiming the damage case impossible.**

**DELIVERED at `97f19b8d`, all three now `Precondition`, exit codes unchanged, the adjacent
public-key-mismatch arm (`trust.rs:335`) still `InvalidSignature` and now pinned in both test layers.**
Review: `.git-exclude/reviewed/rfc147-trust-precondition-sites-review-v1.md`.

**What the round found that the handoff had not:** before it, a three-site variant change **passed the
whole suite** — every trust test asserted `.is_err()` and nothing more, so nothing could tell
`Integrity` from `Precondition`. And site 1's old message was quoted in **four** places, not one:
`beginners_tutorial.rs:97` asserted the exact rendered line and **would have failed in CI**;
`troubleshooting.md`'s H2 **was the old message verbatim, above a body that already described the true
cause correctly** — the docs had the right explanation under the wrong error text for as long as both
existed. The heading now carries the new message; the old text is kept on the page so a search lands.

**Recorded, not actioned — the round's own observation:** the adjacent arm compares two public keys,
so it is arguably not a *signature* check either. It is a key-binding failure rather than a caller
precondition, it was outside the three, and it is now guarded in both directions so a later ruling on
it cannot move silently. Weigh it or leave it; nothing waits on it.

**RULED (2026-09-12, before delivery): fix all three per-site, in RFC 132's established mould.** `rfcs/handoffs/132-error-taxonomy-structure/`
already holds per-site moves (`precondition-variant-handoff-v1.md`,
`six-preconditions-and-the-broad-arm-handoff-v1.md`) that explicitly did not open increment 2, enabled by
increment 1's `#[non_exhaustive]`. §2c's ruling stands — no RFC 132 trigger fires — and **this is not
waiting on this RFC's Case A/B rulings**, which are about *reporting* and are separate. Handoff:
`rfcs/handoffs/132-error-taxonomy-structure/trust-precondition-sites-handoff-v1.md`.

### 2e. RULED 2026-09-12 — Case A: a per-entry field, one shared classifier, and the class fixed with it

**Three facts, checked at source, decide §2b:**

1. **`unsupported-path` is documented as a name category.** `WorktreeChangeKind::UnsupportedPath`'s own
   doc: *"A worktree path could not be safely represented as a Prikk repo path."* Widening it (option 1)
   would redefine a documented, shipped, machine-branchable value. **Rejected.**
2. **The refusal is a class, and status already half-sees it in prose.** `AuthorError` carries
   `UnsupportedSymlinkAuthoring` and `UnsupportedKindTransition` (create, modify, rename and delete of a
   symlink; file↔directory transitions). `worktree_status.rs:172-173` already detects a tracked path that
   is now a symlink or directory — and reports it as **`kind: modified`, detail: "tracked path is not a
   regular file"**. So the tracked case is genuinely *modified* **and** will be refused: **two truths, one
   `kind` slot.** A new `kind` (option 2) cannot carry both. **Rejected.**
3. **Adding a field within `-v1` is settled precedent.** `worktree-status-report-v1` gained
   `declarations` at 0.38.0 (`output/worktree.rs:125`) without a version bump; JSON consumers ignore
   unknown fields by construction. **Additive.**

**RULED: option 3, per entry.** Each worktree change entry gains an orthogonal field — `authoring`, with
values `"authored"` or `"refused"`, and when refused a `refusal` string naming why. `kind` keeps its
meaning; `unsupported-path` keeps its documented meaning. The report gains a count: prose
`refused paths: N`, JSON `refused_count`. **This answers the consumer's actual question — "will this
commit succeed, and on which path?" — per path, which is what client-side prevention needs.**

**The predicate comes from one place.** Status may mark `refused` **only** from a classifier
`node_authoring` itself exposes and uses — one function, both callers. **Two copies of authoring's
refusal rules is how they drift**, and drift here would recreate the exact defect this rules out. RFC
108's own lesson: one shared classifier makes parity a property, not a decision.

**The class travels with it.** `node_authoring.rs:121`, `impl From<AuthorError> for PrikkError`, is where
`UnsupportedSymlinkAuthoring` and `UnsupportedKindTransition` become `Integrity`. **They are
preconditions**, and the round that adds the field reclassifies them in RFC 132's per-site mould — the
messages will be quoted by the new `refusal` strings, so the two must land together. The other
`AuthorError` variants are the round's to judge by the same rule, not to move by momentum.

**DELIVERED 2026-09-12 (`3fd0ed68`).** Reproduced on a rebuilt binary: a tracked file replaced by a
symlink reports `modified … [refused: …]`, `refused paths: 1`, `unsupported paths: 0`, and **`commit`'s
stderr is byte-for-byte `error: ` + the JSON `refusal`**. Review:
`.git-exclude/reviewed/rfc147-case-a-authoring-refusal-field-review-v1.md`.

**CORRECTED — the handoff's premise that status and commit "cannot disagree" was false.**
`From<AuthorError> for PrikkError` carries the *detail* into `Precondition`, not the display line, so
the class change alone would have split the two strings. The round's first build shipped that; its first
verification caught it; the fix renders the refusal *through* `PrikkError::from(..).to_string()`
(`node_authoring.rs:227`). Parts (b) and (c) above were in tension and this section did not say so.

**RULED — three decisions the round raised:**

1. **Struct shape: one `refusal: Option<String>`, two fields on the wire.** The round diverged from
   this section's two-field wording so that `authored`-with-a-reason is unrepresentable. **Accepted.** The
   wire shape is the contract; the struct that cannot lie is the better struct.
2. **`#[non_exhaustive]` on `WorktreeChange`: add it, in the residue round.** This is the fourth public
   struct in three releases to gain a field without it (0.38.0 added fields to four, and the changelog was
   corrected for exactly that). Breaking once; free thereafter. Judge the 0.38.0 four by the same rule —
   report, do not move by momentum.
3. **`NodeIdentityUnavailable` stays `Integrity`.** A property of the repository's history, not a state
   the caller can change — the §2e test applied, not skipped.

**Three blind spots upstream of the classifier, none a divergence in the rule:**

- **G1 (theirs):** a contradicted rename declaration is refused by `commit` and is not a property of any
  path's entry, so `refused paths: 0` **does not mean the next commit succeeds**. Pinned by a test that
  asserts the incomplete truth and fails when a later round closes it; stated in the user's words in
  `worktree-status.md`. Not in scope of a per-path field; a separate ruling if wanted.
- **G2 (theirs):** baseline symlink nodes are skipped at `worktree_status.rs:177` — pre-existing.
- **G3 (the architect's, found by a fixture that differed from theirs by one target file):** a
  **dangling** symlink at a tracked path takes the `Missing` branch — the presence check follows the link
  — so **the classifier is never consulted** while `commit` refuses. A non-dangling link reproduces the
  round's result exactly. Residue handoff.

**G3 CLOSED 2026-09-12 (`979297e9`).** Presence is now non-following — `let Ok(metadata) =
fs::symlink_metadata(&target) else { /* Missing */ }` replaces `exists()` — and **nothing else moved**:
any other metadata failure still reads as absent, exactly as before. A dangling symlink is `modified` and
`refused`, with `missing files: 0` asserted beside `refused paths: 1`; a genuinely absent path is still
`missing`. Commit's stderr equals `error: ` + the JSON `refusal` on the dangling fixture too.

**Ruling 2 DELIVERED — `#[non_exhaustive]` on all five.** The round applied one rule to each — input to
any `pub fn`? constructed outside the crate? — and all five answered *no / none*, verified independently.
**Proven from outside the workspace**: a downstream struct literal of `WorktreeChange` fails
`error[E0639]`; a downstream function reading its fields and `refused_count()` compiles. Breaking once,
on construction only.

**Windows: the field has no test coverage**, stated by the round rather than hidden behind the file's
`#![cfg(target_family = "unix")]` — every refusal needs a non-regular entry no test can create portably.

**`commit --dry-run` (option 4): not now.** With `refused` on every entry, status *is* the dry run for
this class. Recorded as the shape to revisit if a refusal ever depends on state status cannot see.

## 3. Case B — a tag prikk lets you create, then declines to resolve

```
$ prikk tag create tags/v9 --target <block-id>      → exit 0, "created tag tags/v9"
$ prikk checkout --plan-only --ref tags/v9          → exit 1, "object type mismatch: expected block, got tag"
```

### 3a. Checkout already owns the answer, which reframes this

A control stikk did not run: **ask checkout for a ref that does not exist at all.**

```
ref: tags/nope
ref-state: <not published>
materialization: unpublished-ref
note: publish a ref before checkout can target a block
exit 0
```

**Checkout has a designed, machine-branchable vocabulary for "this ref cannot be materialized" — and a
tag does not use it.** An unpublished ref gets an explicit answer and exit `0`; an **existing** tag gets
a type-mismatch and exit `1`.

**So the finding is not "tags fail."** It is that they fail **outside the vocabulary the command built
for exactly this situation**, which is what makes it an inconsistency rather than a policy.

**Three coherent answers**, and the third is not worse than the first:

1. **Teach checkout the `materialization:` vocabulary for tags** — a tag resolves to its target block, or
   reports why it will not, in the same shape as `unpublished-ref`.
2. **Dereference the tag** to its target block and materialize it. More capability than anyone asked for.
3. **Refuse at `tag create --target <block-id>`** if a tag at a block is not meant to be a checkout
   target — **the inconsistency is created there, not at checkout.** stikk says explicitly that "tags are
   deliberately not checkout targets" would be a complete answer.

**Unruled.** stikk is not asking for this and says so; they raised it as an internal inconsistency and
asked that it be weighed apart from their real request.

### 3b. RULED 2026-09-12 — Case B: dereference, through the resolver that already exists; the §6 lean is withdrawn

**The fact that decides it, found by measuring rather than reading:** `refs::resolve_ref_tip_block`
already exists, documented as *the* two-hop ref-tip resolution — *"`Branch` names a Block directly;
`Tag` names a Tag object one hop away"* — **consolidated after the same analysis had been re-derived
three times.** Its callers are `bundle.rs`, `patch_set_digest.rs`, `patch_exchange.rs`. **Checkout and
log are not among them.**

A tag ref driven through the read surface, on a real repository:

| command | result |
|---|---|
| `bundle export --ref tags/v1` | **exit 0, exported** — uses the resolver |
| `checkout --plan-only --ref tags/v1` | exit 1, `object type mismatch: expected block, got tag` |
| `checkout --patch-plan --content-path … --ref tags/v1` | exit 1, same |
| `log --ref tags/v1` | exit 1, **`integrity error: history object … is Tag, expected Block`** |
| `worktree-status --ref tags/v1` | exit 1, `invalid name: ref namespace is reserved` |

**Two sites re-derive the resolution and get it wrong**: `checkout.rs:138-139` takes
`ref_state.target_object_id` as a block; `history.rs:277-279` does the same and calls the result
`Integrity`. Both `checkout` modes flow through the first (`patch_checkout.rs` has no resolution of its
own). **`worktree-status`'s refusal is different and correct** — `validate_local_branch_ref`
(`refs.rs:551-557`) deliberately admits only `heads/`; a worktree baseline is a branch concept, and the
class is `InvalidName`. Untouched.

**RULED: option 2.** Route `checkout.rs:138` and `history.rs:277` through `refs::resolve_ref_tip_block`.
A tag reads as its target block in `log`, `checkout --plan-only`, `--patch-plan`, and `--content-path`.
No new capability is invented — the resolver, the model (ref → tag object → block, per `tag.rs`'s own
module doc) and the working precedent (`bundle`) all exist; this is the fourth and fifth call site
joining the first three. **`history.rs`'s `Integrity` on a tag disappears with the fix rather than
needing its own reclassification.**

**§6's lean — option 3, refuse at `tag create` — is withdrawn.** It would break `bundle export --ref
tags/x`, which works today, and it would contradict the tag model to protect a resolver two commands
never adopted. **Option 1 is subsumed**: with dereferencing, `materialization:` reports the target
block's real state, which is the vocabulary §3a asked for.

### 3c. DELIVERED 2026-09-12 (`d00d7768`, `7c9c06f3`) — and §3b's site inventory was wrong twice

**Delivered for the four scoped surfaces**, proven on a **non-tip** tag: `log` shows history from the
tagged block; `checkout --plan-only` names it; `--patch-plan --content-path` returns **its** content
(`first`, not the tip's `second`); a **received** tag resolves to the tagged block after `bundle import`.
`worktree-status` and `bundle export` unchanged. Reviews:
`.git-exclude/reviewed/rfc147-case-b-tag-ref-resolution-review-v1.md`.

**CORRECTED — §3b named two sites; there were four, and then a fifth.** The round found, by confirming
rather than trusting §3b's claim, that `--patch-plan` and `--content-path` never reach `checkout.rs` —
they reach **`patch_replay/read.rs::current_target_block`** — and that `history.rs` has **two** loops
(`load_ref_history`, `load_received_ref_history`). All four now call `refs::resolve_ref_tip_block`. **One
of the four had no test until the round perturbed it** and found the whole suite green; they wrote the
test (`7c9c06f3`) rather than only reporting the gap.

**Then the round claimed one more than it fixed.** *"`patch_inverse` shares `current_target_block`, so it
resolves a tag ref now as a consequence"* — **false on the binary**: `inverse-plan --ref tags/v1` still
fails with `object type mismatch`. **There are two functions named `current_target_block`**,
`patch_replay/read.rs:21` and `patch_inverse/read.rs:16`, each `pub(super)` in its own module. A name in
two modules is two functions.

**RULED — §3b's "unreachable from a valid tag ref" is scoped to the four surfaces.** The residue,
measured across every `--ref`-taking read command: **`inverse-plan` and `rollback-preview` still
mismatch** (the unfixed duplicate); **`merge-evidence` and `merge-plan` refuse tags deliberately**, by
`validate_local_branch_ref`, the same branch-only rule as `worktree-status`, class `InvalidName` —
correct and untouched. The duplicate goes to the residue handoff.

### 3d. RESIDUE DELIVERED 2026-09-12 (`979297e9`, `d1b6be83`) — fifth site fixed, census done, two semantics ruled

**`patch_inverse/read.rs` now calls the resolver**; `inverse-plan`, `rollback-preview` and
**`rollback-draft-verify`** (a third command, hidden behind an empty-WAL refusal until a draft exists)
resolve a tag to its target block. Review: `.git-exclude/reviewed/rfc147-residue-review-v1.md`.

**The census, done by driving 19 `--ref` invocations rather than by reading the call graph:** four
resolve (§3b), three fixed here, `snapshot-plan` resolves then fails on a real fact, eight refuse by the
branch-only validator **on purpose**, and two still mismatch. **§3b/§3c's inventory is now closed at the
command level**, which is the level a consumer meets.

**RULED — the two that still mismatch:**

- **`branch create heads/<n> --from tags/<t>`: resolve.** `--from <ref>` ordinarily means "at the block
  this ref names"; every read surface now dereferences; the operation reads the tag and writes a new
  branch. Same resolver.
- **`tag create <n> --target tags/<t>`: refuse explicitly, as `Precondition`, naming the accepted forms
  (a block id, or a branch ref). Do not dereference.** The model is ref → tag object → block, one hop; a
  tag-of-a-tag is outside it; collapsing silently to the block would make two different requests
  indistinguishable in history. The round declined to act without a ruling, and was right to.
- **Consolidate the two `current_target_block`s into one.** An imperative comment binding two
  near-identical bodies is a comment doing a compiler's job.

Handoff: `rfcs/handoffs/147-refusal-visibility/tag-target-semantics-handoff-v1.md`.

**Carried, not acted on:** `snapshot-plan`'s *does not contain a snapshot blob* renders as `Integrity`
for a by-design absence (RFC 136 §7) — same class as the sites already moved; a later per-site pass.

### 3e. DELIVERED 2026-09-12 (`df3c57eb`) — the §3 arc is closed

**All three §3d items landed.** `branch create --from tags/<t>` resolves — **a sixth site, the first
outside `prikk-store`** (`branch.rs::resolve_published_target`), with its existence check moved onto the
resolved block. `tag create --target tags/<t>` refuses as `Precondition` on the kind the CLI already
held, naming both accepted forms, and both forms are proven to create. The two `current_target_block`s
are one: **`refs::read_current_ref_tip_block`**, placed in `refs` rather than either sibling so that no
lateral edge is invented — both `read.rs` files now mention `refs` zero times, the dependency having
moved up to parents that already carried it. Review:
`.git-exclude/reviewed/rfc147-tag-target-semantics-review-v1.md`.

**One root export added**: `resolve_ref_tip_block` is now `pub` and re-exported from `lib.rs`, chosen
over a seventh hand-written copy of the two hops in `prikk-cli`. The next release's API diff shows
264 → 265 and should.

**Coverage of the one resolver, measured by breaking it under `--no-fail-fast`**: 184 tests across 91
binaries reach it; three pin the tag hop specifically.

**Nothing remains in §3.** Every `--ref`-taking command either dereferences a tag, refuses it by the
deliberate branch-only validator, or — `tag create --target` alone — refuses it as a precondition
because the model is one hop. **Carried out of this RFC**: `checkout --snapshot-plan`'s `integrity
error` on a by-design missing snapshot, a per-site class fix for a later pass.

**A byproduct worth naming:** this gives the stikk project **block-addressable content for any tagged
block** — the `--ref tags/<name>` row of their RFC 144 §4t table stops failing. It does not answer bare
`--ref <block-id>`, which stays RFC 144's question.

## 4. What this RFC does not cover

- **Block-addressable content** (stikk letter 006 §5) — a real request, larger than these two, and
  **RFC 144's territory** rather than this one's. Recorded there.
- **`CreateSymlink` authoring.** Out of scope by existing design; this RFC is about *reporting* a
  refusal, never about removing one.
- **Any error-message rewording** beyond §2c's classification point.

## 5. Non-goals

- **No new capability.** Both cases are about telling a consumer something prikk already knows.
- **No silent redefinition of a shipped `kind` value** (§2b option 1 is listed, not preferred).
- **No `--dry-run` by default.** If §2b lands on option 4, it is its own increment with its own design.

## 6. The architect's lean, stated so the owner can disagree with something concrete

**SUPERSEDED 2026-09-12 by §2e and §3b, which rule on measured facts.** Kept so the reversal on Case B is visible: the lean below preferred refusing at `tag create`; the facts preferred the resolver that already existed.

- **Case A: option 2 or 3, not option 1.** A shipped machine-branchable value should not gain members
  silently, and a consumer's real question — *"will this commit succeed?"* — is about the commit, not
  about one path's kind. **Option 3 answers the question actually being asked**; option 2 is the smaller
  change and answers most of it.
- **Case A's §2c: fix the class.** `Precondition` is what both sites mean, the variant already exists,
  and two independent instances in one day is enough to act on.
- **Case B: option 3.** If a tag at a block is not a checkout target, **the refusal belongs at creation**,
  where the user can still do something about it — not three commands later.

## 7. Revisit triggers

- **A consumer branches on `unsupported-path` in the field.** Today none is known to; that is what makes
  §2b option 2 cheap and it would stop being true.
- **`CreateSymlink` authoring comes into scope.** Case A's whole premise is that the refusal is
  permanent.

## 3f. Case C — unrepresentable names (2026-09-13, from stikk's letter 010)

Measured by stikk on 0.41.0 and 0.28.0 and reproduced here: an entry whose name cannot be a repository
path (`back\slash.txt`, a non-UTF-8 name) is `kind: unsupported-path` with `authoring: "authored"` and
`refusal: null`, while `commit` refuses it with `invalid name: …`. §2e(b) kept the kind's meaning
("unrepresentable name") correctly and left the verdict wrong — the `Err` arm holds the exact error
`commit` prints and discards it. **RULED:** the entry carries that error as its `refusal` and is
`refused`; and its `path`, absolute today, becomes the OS name relative to the worktree root, rendered
lossily, with `worktree-status-report-v1`'s description saying so for this one kind (stikk's second
question). Handoff: `147-refusal-visibility/unrepresentable-names-handoff-v1.md`.

**§3f DELIVERED 2026-09-13 (`bf36b2ee`, `804b3b51`)**: the entry carries `commit`'s own `invalid name` refusal (same two functions, root-relative path), is `refused`, and its `path` is root-relative; `worktree-status.md` states the one-kind exception. Measured by the architect on the binary.

## 2f. RULED 2026-09-16 — G1: a declaration reports what `commit` will do with it

Arises from stikk's letter 013
(`.git-exclude/upstream/stikk/receive/013-a-declared-rename-cannot-say-whether-its-content-changed.md`).
§2e left G1 open ("needs a shape ruling first"). The architect measured every case on a binary built
from current code (`47fb6def`; later commits touch no code), in fresh repositories, with `a.txt` and
`keep.txt` sealed and then `prikk mv a.txt b.txt`:

| Worktree afterwards | `worktree-status --format json` | `prikk commit` |
|---|---|---|
| unchanged | `missing a.txt`, `untracked b.txt`, declaration `a.txt → b.txt` | `rename-path` |
| `b.txt` edited | **byte-identical to the row above** | `rename-path` **and** `edit-text` |
| `b.txt` deleted | `missing a.txt`, declaration still listed | `delete-file a.txt`, declaration named as a deletion |
| `a.txt` recreated beside `b.txt` | `untracked b.txt` `"authoring": "authored"`, `refused_count: 0` | **refused**: "…move b.txt back to a.txt to clear the declaration…" |
| shell `mv b.txt a.txt` | **`clean: true`, `changes: []`, `refused_count: 0`**, declaration still listed | **refused, with the same advice** |

**The loop.** The refusal's advice (`node_authoring.rs:499-501`) and the status note
(`output/worktree.rs:82-84`) both say to move the destination back to the source. Done in the shell, that
produces exactly the state the refusal names, so the user loops. What actually clears it, measured:
- **Source back, destination gone:** `prikk mv b.txt a.txt` ("nets to no move, dropped"), or
  `prikk mv a.txt b.txt` (redoes the move; commit authors the rename).
- **Both present:** `prikk mv` refuses in both directions ("both paths exist"). Setting aside the copy
  that is not wanted is the only way out. Removing the recreated `a.txt` let commit author the rename.

§4o.2 of RFC 144 rules the refusal itself; it is not reopened. The advice is what is wrong, and a status
report that says `clean` before a refused commit fails §2e's own question: *"will this commit succeed?"*

**RULED 1 — per declaration, from commit's own classifier.** Each `declarations` entry gains:
- `resolution`: `"rename"` | `"deletion"` | `"deletion-ignored"` | `"never-tracked"` | `"refused"`;
- `refusal`: `null`, or commit's own message.

It must be computed by **one function commit also uses**, evaluating the whole batch (the two-node swap
tolerance at `node_authoring.rs:452-460` included), never re-derived in status. The fields sit per
declaration, not per path, because the shell-move-back state has **no path entry at all** to mark. The
report gains `refused_declaration_count`, and prose `refused declarations: N`. `refused_count` and
`clean` keep their documented path-level meanings, and the docs say that a refused declaration can sit
in a `clean` worktree. Additive within `worktree-status-report-v1`.

**RULED 2 — every refusal names a command measured to work in that state.**
- Source back, destination gone: `prikk mv <new> <old>` to drop the declaration, or
  `prikk mv <old> <new>` to redo the move.
- Both present: keep one copy (set the other aside); then commit, or `prikk mv <new> <old>`.
- Destination occupied by another tracked node: the route is measured by the round before it is quoted.

The status note stops advising a shell move. A control runs the command each refusal names and must fail
against today's text.

**RULED 3 — a rename says whether content or mode also changed.** On `resolution: "rename"`, add
`content_changed` and `mode_changed` (booleans; `null` for every other resolution). They come from the
comparison commit uses to author the accompanying `edit-text` or mode operation, not a second read path.

**Scheduling:** after RFC 136 increment 3's review; **0.43.0 does not cut without it**, because stikk
re-baselines on 0.43.0 and the advice today strands a user. Handoff:
`rfcs/handoffs/147-refusal-visibility/declarations-say-what-commit-does-handoff-v1.md`.

**DELIVERED 2026-09-16** (`ee86aebe`, `e1f7a412`; reviewed
`.git-exclude/reviewed/declarations-say-what-commit-does-review-v1.md`). One classifier,
`crates/prikk-store/src/declaration_resolution.rs`, is called by both `commit` and `worktree-status`;
each declaration carries `resolution`, `refusal`, `content_changed` and `mode_changed`, and the report
carries `refused_declaration_count`. Additive within `worktree-status-report-v1`, confirmed by diffing a
pre-round binary's document against this one on the same repository: keys added, none removed or renamed,
no shared value changed. Verified by the architect on a binary of `e1f7a412`: every state of §2f's table,
each refusal byte-equal to commit's own message, every route each refusal names run in the state that
produced it (five routes, each leaving no refusal standing and a file with the content), the two-node swap
resolving as two renames, and `clean: true` alongside a refused declaration.

**The round's own control caught a message that named `prikk mv <new> <old>` where `prikk mv` refuses
(both paths present)** — the class of error this ruling exists to prevent, caught by the required control
rather than by a reader.

**Three questions ruled at review:**
1. **`worktree-status`'s exit code is unchanged.** It means "the worktree differs from its baseline"; a
   refused declaration is not a difference (the undone-move state matches the baseline byte for byte).
   Overloading it would break cleanliness checks, and both output formats now carry the fact.
2. **Parity rows for `never-tracked` and `deletion-ignored`** — measured correct at review, uncontrolled on
   the status side — **go into the 0.43.0 prep round** as regression protection.
3. **`prikk mv --drop-declaration`** is a candidate for the owner's scheduling, not this round's work; the
   refusal explains the manual route.

## 2g. RULED 2026-09-16 — presence is one definition, because §2f's two views disagreed

stikk's letter 014 measured a state §2f's parity claim did not hold in: with a **directory** at a declaration's
destination, `worktree-status` resolved `rename` while `commit` recorded a deletion and called the destination
*ignored*. Reproduced by the architect on the released 0.43.0 asset, empty directory and non-empty alike.

**Where §2f went wrong.** It ruled one classifier "called by both callers with their own baseline and presence
views". The classifier was shared; the definition of *present* was not. Status counted any entry on disk,
commit counted regular files from its walk, and `resolve_one` then read "on disk but absent" as "ignored". §2f's
review verified parity across its table's states — all regular files — and never varied the entry kind. **The
architect's miss.**

**RULED:**
1. **Presence is decided inside the classifier, by commit's own entry classification** — never by a
   caller-supplied closure.
2. **A directory at the destination resolves `deletion`**, and commit's disclosure names it
   (*"destination is a directory; recorded as a deletion, not a rename"*).
3. **`deletion-ignored` only when the ignore rules exclude the destination.**
4. **No new resolution value** in the patch release; `deletion`'s documented meaning widens to cover a destination
   that is no longer a file.
5. **Symlinks and special files are measured, and kept or reported**, not assumed.

A parity matrix across entry kinds becomes the control. Handoff:
`rfcs/handoffs/147-refusal-visibility/declaration-presence-is-one-definition-handoff-v1.md`. Ships in 0.44.0.

## 2h. RULED 2026-09-16 — a destination that is not a regular file is never read, and reports no difference

The §2g round stopped at its own rule 5, correctly. Measured on the 0.43.0 code with a declared move whose
destination was replaced:

- **by a FIFO: `worktree-status` never returns.** `destination_differences` computed `content_changed` with
  `std::fs::read`, which opens the FIFO and waits for a writer that never comes. Commit, meanwhile, refuses
  cleanly (`worktree entry is not a regular file`). **Shipped in 0.43.0, from §2f's round** — the architect
  ruled that field and reviewed it with a regular file at every destination.
- **by a Unix socket:** `rename` with `content_changed: false` — the read failed and was reported as "no
  difference".
- **by a symlink:** `rename`, with `content_changed` computed through the link, while commit refuses over the
  path.

Commit's own walk sorts every non-directory, non-regular entry — symlink, FIFO, socket — into "refuse the whole
commit over that path", before any declaration is resolved.

**RULED — option (A):**
1. **A FIFO or socket at the destination resolves like a symlink:** `rename`, with commit refusing over the path,
   and that refusal carried by the path's own entry, as §2e already reports it. **Not** `deletion`: commit does
   not record a deletion there, and a report saying so would be §2f's disagreement in a new shape.
2. **`content_changed` and `mode_changed` are `null` whenever the destination is not a regular file** —
   symlinks included, so a symlink's value changes from a comparison through the link to `null`. Commit never
   authors through a symlink, so a value computed through one described nothing commit does. The documented rule
   becomes: `null` unless the resolution is `rename` **and** the destination is a regular file.
3. **A destination that is not a regular file is never opened.** Presence and kind come from one non-following
   stat; bytes are read only for a regular file. That is the hang's fix, and it holds for every caller.
4. **§2g's rules 1–4 stand unchanged**: presence decided once, by commit's own classification; a directory
   resolves `deletion` with its own disclosure; `deletion-ignored` only from the ignore rules; no new
   resolution value.

The parity matrix gains FIFO and socket rows, each run under a timeout so a regression fails instead of hanging
the suite. **CHANGELOG `### Fixed`:** `worktree-status` hung on a FIFO at a declared destination (0.43.0), and a
directory there was reported as a rename. **`### Changed`:** `content_changed`/`mode_changed` are `null` for a
destination that is not a regular file, symlinks included.


## 2i. RULED 2026-09-22 — an unpublished current branch is not an absent ref

**From stikk's letter 015**, and **reproduced by the architect on the released 0.46.0 binary**, one fresh
repository, one untracked file, nothing sealed:

| invocation | 0.44.0 | 0.46.0 |
|---|---|---|
| `worktree-status` (no `--ref`) | the report, exit 0 | **the report, exit 0** |
| `worktree-status --ref heads/main` | the report, exit 0 | **`precondition not met: ref heads/main does not exist in this repository`, exit 1** |
| `log --ref heads/main` | empty history, exit 0 | the same refusal |
| `tree --ref heads/main` | *(new in 0.46.0)* | the same refusal, while bare `tree` prints `target block: <not published>`, exit 0 |
| `diff --from heads/main` | *(new in 0.46.0)* | the same refusal, while a bare `diff` compares against the empty state |
| `status` | — | prints `heads/main RefState: <not published>` **and** `current branch: heads/main` |

**The refusal is false.** `heads/main` is the repository's current branch: `status` names it, `commit` authors to
it, and every reader answers for it when it is *not* named. Naming the same ref, in the same repository, at the
same moment, cannot make it stop existing. The refusal sweep's `require_existing_ref` conflated **"has a published
`RefState`"** with **"exists"**, and for the current branch those differ until the first `seal`.

**Ruled:**

1. **A named ref that is the repository's current branch is read, published or not**, and answers **exactly what
   the implicit path answers** — the empty state, exit 0. The reports already say which it is
   (`<not published>`), so nothing is hidden by reading it.
2. **Every other ref with no `RefState` keeps the refusal.** A mistyped branch name is the case the sweep was
   written for, and it is untouched. So is a received ref, and so is a damaged one (`integrity error: ref <ref>
   is not published`, RFC 153's own change — note the collision of *words*: damage says "is not published" of a
   ref whose pointer is gone, while an unpublished branch is simply new. The fix must not make those two answer
   alike).
3. **It applies to every reader that takes a ref or a point**, because the seam is the resolver, not the command:
   `worktree-status`, `log`, `tree`, `cat`, `diff --from`, and `checkout`'s read-only modes. The parity to assert
   is *implicit equals explicit*, per command, on a fresh repository.
4. **RFC 157 §3 and RFC 153 §7.3 are corrected by this:** their "an explicit absent `--ref` refuses" stands only
   for a ref that is genuinely absent. The current branch named explicitly is not that ref.

**A third voice, recorded and not ruled:** in the same state `prikk branch list` prints `no branches` while
`status` prints `current branch: heads/main`, and `branch create heads/side` refuses with *"ref heads/main does
not exist in this repository"* — so a second branch cannot be made before the first seal. Whether `branch list`
should name an unpublished current branch is a question for the consumer that parses it; it is asked in the reply
to stikk 015 and is **not** part of this fix.

**Why this is the architect's defect, not the sweep round's:** the sweep did exactly what its handoff said, and
the handoff said "an explicitly named absent ref refuses" without ever asking what *absent* means for a branch
that has never been sealed. The same unexamined word then went into RFC 157 §3 and RFC 153 §7.3, which is why the
seam widened in 0.46.0 instead of being noticed.

### 2i.1 — DELIVERED 2026-09-22

`6dee6c4f` (the fix) and `7405ec3b` (its follow-up), reviewed as `unpublished-current-branch-review-v1` and
`…-follow-up-review-v1`; gates 14/14 on each, re-run by the architect, and both rounds' choke points perturbed
independently.

**Three corrections the rounds made to the architect's own instructions, all right:**

1. **The fix does not belong in the shared resolver.** `require_existing_ref` is called both by readers and by
   `checkout`'s materialize modes as a **pre-write gate**; relaxing it would have made those stop refusing, and
   the shared version read the current-branch pointer from `prikk-store`, which **RFC 151 §2.2 forbids**. Two
   existing tests caught both. The shape that shipped is a store predicate that never reads the pointer
   (`is_unpublished_local_branch`) plus one CLI function that does, through the single allowed reader —
   `decide_ref_existence` is byte-for-byte unchanged, so no unlisted caller moved.
2. **`--from <the unpublished current branch>` must not fold the queue.** "Byte-identical to the bare form" was
   right only with an empty queue; with a queued commit it made `diff` say the point already held a file that
   `tree` and `cat` said was not there. `--from` now compares against the point exactly, so it means one thing
   on both sides of the first seal, and `WorktreeDiffFrom` gives the left side three named states instead of
   overloading `None`.
3. **`diff --format json` bare and explicit were never byte-identical**, even with an empty queue: the implicit
   side always carries `queued_patches: 0` and an explicit point carries no such field. The exception is named
   in the control and pinned by its own test rather than dropped from a list.

**And `cat`'s parenthetical** now reads `(<branch> has no published history yet)`, caught by **variant** with
the resolver's message discarded — so the false wording cannot be re-inherited by a later refactor. RFC 157
Addendum 3 item 2's pinned string is superseded for this one case and unchanged for every other.

## 2j. RULED 2026-09-22 — `branch switch` names a route that refuses

Found by the architect while measuring stikk 017's `branch list` question. On a fresh repository, **on the
released 0.46.0 binary and on `main` alike**:

```text
$ prikk branch switch heads/main
error: precondition not met: heads/main does not exist; run `prikk branch create heads/main` first

$ prikk branch create heads/main          # the route it just named
error: precondition not met: ref heads/main does not exist in this repository
```

**The route the refusal names cannot work**, and the refusal is false twice over: `heads/main` is the current
branch — `status` says so — and switching to it is not something the user needs to do at all.

**Ruled:** `branch switch <the current branch>` says that it is already the current branch, and, when it has no
published history, that the first `seal` publishes it. It does **not** name `branch create`. Every other absent
target keeps today's refusal, `branch create` included — creating a second branch before the first seal is
genuinely impossible, and that refusal is true.

Same family as §2i and the same root: a state the product supports (a branch that exists and has never been
sealed) described by code that only knew *published* and *absent*. Third face of one unexamined word — the
readers (§2i), the listing (RFC 146 §8f), and now the one writer that points at a dead route.
