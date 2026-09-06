# RFC 141 §7b — the ruling you asked for: update the one oracle case, and commit

**Amends:** `tighten-the-evidence-schema-handoff-v1.md`. **v1 stands in every other respect** — this
answers one question and adds one requirement.
**RFC:** `rfcs/accepted/141-publication-through-ci.md` §7b.1 carries the ruling and its reasoning.
**Base:** your existing working tree on `main` at `c287b9b`.

---

## 1. You were right to stop, and v1's §3 was wrong

**The pre-check in v1 §3 was mine and it was wrong.** The pack stores each document as a JSON
**string** in `content`; my scan walked dicts and lists, descended into nothing, and printed
*"crate rows scanned: 0"* — which I read as "no dishonest rows" when it meant "I scanned nothing."
**Zero findings over a zero denominator is not a measurement.**

Re-scanned properly: **623 crate rows, exactly one dishonest**, and it is the entry you named. Your
count is confirmed independently. **Stopping and reporting rather than editing was exactly right**,
and it is what made the error visible in one round instead of after it had been papered over.

## 2. RULED: the three-field edit is in scope. Apply it and commit.

`release/oracle/oracle-manifest-v1.json`, case `release-evidence:pending_false_checksum_match`:

- `expected.structural`: `"valid"` → `"invalid"`
- `expected.semantic`: `"invalid"` → `"not-run"`
- `expected.primary_reason`: `"evidence-tag-or-artifact"` → `"schema-instance"`

**This is the correct completion of the change, not an accommodation of it.** v1 §3 forbade editing a
fixture's **content** or its **verdict** to make a suite green. This edits neither: the document is
untouched and its verdict is unchanged — `invalid` before, `invalid` now. What changes is the recorded
**layer attribution**, which is what those fields exist to record, and the constraint deliberately
moved between layers.

**The values are not invented**: `structural: invalid` + `semantic: not-run` + `primary_reason:
schema-instance` is an existing triple carried by **17 of the 57 cases**. This case joins them.

## 3. One requirement I checked before ruling, and you must keep true

**Tightening the schema must not orphan the Rust equality check.** The schema can only constrain
*presence*; `crate_checksum_state_valid` also checks that present values actually **agree**.

Verified: **one case still exercises that half** — `complete_checksum_mismatch`, which claims
`"match"` over three present-but-**unequal** values. No JSON Schema can reject that; only the Rust
check can. **Had it not existed, this ruling would have required adding it.**

**So: confirm that case still passes and still reaches the semantic layer after your edit.** If your
change causes `complete_checksum_mismatch` to be caught structurally instead, the equality check has
no exercised coverage left and that is a finding — stop again and say so.

## 4. Then commit, in one commit

Everything else in your report is accepted as it stands, including:

- the `$defs/crate` conditional and its `description` stating the shape/agreement split;
- **the `"description"` keyword added to `schema.rs`'s `KEYWORDS` allowlist** — a deliberate,
  reasoned widening of a preflight allowlist for a pure annotation keyword with no validation effect.
  Correct, and correctly flagged rather than slipped in;
- the manifest's `normative_schema` identity and all 57 per-case pinned schema identities, updated
  because `check`'s own `verify_file` integrity gate caught the edited file. **That gate doing its job
  is worth more than the inconvenience it caused.**

**Re-run the full gate set against the final commit** — all ten must be green, including `check` and
both `cargo test` invocations.

## 5. Your equality-half question — answered, and not this round's work

**Your reasoning is right and it is on record now** (RFC 141 §7b.1's requirement is the near-term half
of it). A producer that can *see* a mismatch is arguably obliged to refuse rather than record it —
but that changes `produce`'s contract, and today `produce` recording `checksum_equality: "mismatch"`
honestly is a defensible behaviour, not a bug.

**Not this round, and not increment 4's either without its own decision.** The trigger you named is
the right one: once §7a's `CRATE_ORDER` blocker is resolved and a real produced document actually
passes through `policy::evidence`, the case gets much stronger. **Raise it again then.**

## 6. Reporting

Amend your existing report or add `-v2`, whichever is cleaner. Include only: the
`complete_checksum_mismatch` confirmation from §3, and the final gate set against the commit.
