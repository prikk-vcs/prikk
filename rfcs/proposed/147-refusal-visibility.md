# RFC 147 — What the read surface promises about what a write will refuse

**Status.** **PROPOSED 2026-09-10**, on two reports from the stikk project (letter 006 §4, §6),
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
