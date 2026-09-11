# RFC 132 — three trust-path preconditions reported as `Integrity` / `InvalidSignature`

**Ruled:** `rfcs/proposed/147-refusal-visibility.md` §2c/§2d, under RFC 132's standing per-site practice
(`precondition-variant-handoff-v1.md`, `six-preconditions-and-the-broad-arm-handoff-v1.md`). **This
does not open RFC 132 increment 2 and does not wait on RFC 147's Case A/B rulings** — those are about
reporting; this is three message-and-variant sites.

**Your own §3a/§3b diagnosis, promoted, plus one more.** Every location below was opened at source by
the architect after your report; they are yours to re-open, not to trust.

## 1. The three sites

1. **`trust.rs:214-217`, `load_maintainer_trust_policy`.** `read_current_trust_policy_snapshot(layout)?
   .ok_or_else(|| Integrity("publication trust policy is missing or unreadable"))`. The `?` already
   propagates a real read failure; `trust_index.rs:533` already says *"damaged snapshot; run doctor"*
   for the damaged case. **The `ok_or_else` arm fires on `Ok(None)`: nothing adopted yet.** Every second
   project hits it (`first-run.md`'s new section now documents the flow that reaches it).
2. **`add_trusted_maintainer`, the collision arm** — `InvalidSignature("maintainer key id {id} is
   already adopted with a different public key")`. **No signature was checked.** The same-key arm beside
   it is idempotent, which is why `setup` on an existing repository always collides: fresh material,
   fixed id `maintainer`.
3. **`seal`, signer id not in policy** — `InvalidSignature("maintainer signer key id {id} is not trusted
   by policy")`. **Policy membership, not a signature.** Found by the architect while re-running your
   `--key-id`-must-match sequence. **Locate it yourself**; the line number was not opened.

## 2. What to do

**Variant: `Precondition`, all three.** The variant exists; RFC 132 increment 1 made adding nothing
necessary.

**Message, site 1 — a wording decision, and you named why.** `Ok(None)` is also what a container
truncated below its own header replays as. **Lead with the common cause and the step**, and do not
claim the narrow case impossible:

> `no maintainer key is adopted in this repository yet; run \`prikk trust maintainer add\` (a trust
> policy container that replays empty reads the same way — run \`prikk doctor\` if a key was adopted
> here before)`

Shorter is better if it keeps both halves. **The second half is the honesty clause; do not drop it to
save a line.**

**Messages, sites 2 and 3**: keep the facts they already state; the variant and the `error:` prefix
class are what change. Site 2 should say what to do — `prikk key generate` for a new id, or export the
existing seed — in one clause.

## 3. Controls

1. **Perturb each site**: for each of the three, a test that reaches it and asserts the variant, then
   the variant flipped back to the old one and the test shown failing. **Three sites, three failures,
   three restores** — the RFC 132 rounds established this and one green run does not discharge it.
2. **Reach site 1 by the documented route** — `init`, no trust, `seal` — not by a mocked snapshot.
3. **The CLI's rendered prefix** must follow the variant (`error:` classes per RFC 121). Check the
   rendered line, not only the `PrikkError` value.
4. **Exit codes unchanged** — RFC 121's `1` for all three, before and after.
5. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; **state the cross-target addendum's
   outcome** — `trust.rs` counted at source.
6. **`first-run.md`'s captured outputs quote site 1's old message.** If the message changes, **the page
   must change in the same round**, or it ships stale within hours of being written. Grep the docs
   tree for every message you touch.

## 4. What this round must NOT do

- **No new variant.** `Precondition` exists.
- **No change to what any of the three sites *does*** — only what it says and how it classifies.
- **No touch to the `?`-propagated paths or to `trust_index.rs:533`** — those are already right.
- **No RFC 132 increment 2.**
