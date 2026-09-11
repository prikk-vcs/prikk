# RFC 147 — What the read surface promises about what a write will refuse

**Status.** **PROPOSED 2026-09-10; both cases RULED by the architect 2026-09-12 (§2e, §3b) on the owner's instruction — acceptance is the owner's.** Opened on two reports from the stikk project (letter 006 §4, §6),
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
