# RFC 142 follow-up — degrade absence, propagate error

**RFC:** `rfcs/accepted/142-content-surface.md` — **§6b is new, added 2026-09-08.** It corrects §6a's
requirement 4, which was mine and was too broad. **Read §6b before anything else.**
**Base:** `7f603fc`, accepted in shape and held unpushed. Commit on top.
**Round 2 review:** `.git-exclude/reviewed/142-show-degradation-review-v1.md`.

---

## 1. What is accepted

The infallible `show_operation` signature, `ShowBlobContent::Unavailable`, folding `ReplaceBinary`'s
two sides into the same type, the docs, the CHANGELOG, and all ten gates re-run independently against
`7f603fc` (1711 passed, 0 failed). §4's observation — that *which operation kind* degrades is itself
evidence, because seal-time validation closes three of the four sites — is a real finding and is now
part of how this surface is understood.

**Keep all of it.** This round changes one decision inside `show_blob_content`.

## 2. The defect, and it follows from my wording

§6a requirement 4 told you: if you cannot separate "unbacked by design" from "store damaged", degrade
both. **You implemented that as written. It is true of absence and of nothing else, and I did not say
so.**

`prikk show` on a repository with a corrupted blob now exits `0` and renders `<unavailable blob …>`.

`Err(_)` in `show_blob_content` swallows, among others:

- **`Integrity("index entry for {id} resolves to an envelope with computed id {computed}")`** —
  `object_store.rs:130-135`, the content-hash check. **This is silent-corruption detection.**
- `Integrity(...)` for index/envelope type disagreement — `:136-141`
- `ObjectTypeMismatch` from `read_typed`
- `validate_read_schema` rejection
- `BlobPayload::decode_canonical` failure
- any `Io` error from a failing disk

**This is round 1's defect with its sign reversed.** Round 1 called an intact repository damaged;
this calls a damaged repository intact, and the second is worse because the first was loud.

## 3. RULED — §6b

- **`Ok(None)` → `ShowBlobContent::Unavailable { blob_id }`, exit `0`.** DC-65's case. §6a is
  unchanged here and everything you built for it stands.
- **`Err(_)` → propagate. Exit `1`** (RFC 121: operational failure).

**The structural cause is one line, and the fix is there.** `read_blob` collapses
`Result<Option<ObjectEnvelope>>` into `Result<_>` with `.ok_or_else(...)`, destroying the distinction
before its caller can branch. **Keep the `Option`.** Everything needed is already coming out of the
object store.

**`show_operation` need not become fallible again to do this** — but if the cleanest expression is a
`Result` at that layer, take it; the type-level "never fails" was a nice property, not a requirement,
and correctness outranks it. **Say which you chose and why.**

**`BlobKind::Snapshot` goes back to being loud.** A file-content operation naming a SNAPSHOT blob has
no by-design cause; it was an `Integrity` error before this round and should be one again.

## 4. Controls

Each seen to fail before it passes.

1. **A corrupted object fails loudly.** Build a real repository, corrupt the stored bytes of a blob a
   `CreateFile` names so the id recomputation at `object_store.rs:130-135` disagrees, and show that
   block: **exit `1`, and the message names an integrity problem.** This is the control that would
   have caught this round; build it first.
2. **`Ok(None)` still degrades.** The `EditText`-then-`DeleteNode` sequence still exits `0` and still
   renders every operation. **Round 2's controls 1 and 2 already do this — keep them passing
   unchanged.** They bite: I perturbed `Err(_)` to an empty `Text` and both failed, along with two
   `show/tests.rs` tests.
3. **Each error class is distinguished from absence**, at the `prikk-store` level: absent → degrade;
   type mismatch, malformed payload, and `Snapshot` kind → error. Your existing four-site test
   constructs only the absence case; **it needs an error-case sibling, and the two must assert
   different outcomes.**
4. **`ReplaceBinary`'s JSON shape is asserted directly.** Not via a block that lacks one.

## 5. Control 4 could not fail, and that is the thing to take from this round

Round 2's control 4 asserted that an **edit block's** JSON contains no `unavailable`. An `EditText`
dereferences no blob, so that assertion passes no matter what the fix did. Meanwhile the one output
that *did* change shape — `ReplaceBinary`'s JSON, `old_blob_id`/`old_size`/`new_blob_id`/`new_size` to
nested `old`/`new` — was untested by it and unmentioned in the report.

**The shape change itself is fine and stays.** `show` is unreleased and one idiom beats two.

**This is the second consecutive round where a control was satisfied by a fixture that could not
exercise it** — round 1's control 3 was rebuilt with `ChangePerm` specifically to avoid the blob
path. Both times you named the substitution in the report, which is the only reason both were
recoverable, and I would rather have that than a quieter round.

**The rule: when you change a fixture, re-ask whether the control can still fail.** If the answer is
no, it is not a control any more — say so and rebuild it, or say why the property is unreachable.

## 6. Documentation

**`docs/src/guide/show.md` must narrow with the ruling.** It currently tells a reader that `show`
cannot tell an unavailable blob apart from repository damage. After this round it can, and does:
unavailable means the object is **absent**, whose overwhelmingly common cause is DC-65, and damage
now fails the command. Rewrite that paragraph and the Claim-to-Source Anchors row.

`CHANGELOG.md`: still one `## Unreleased` `show` entry, folded, for the same reason as last round.

## 7. Gates

The full set, verbatim from `rfcs/EXECUTION-ORDER.md` §6 rule 9:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo +1.85.0 test --workspace --locked`
- `cargo +1.85.0 check --workspace --all-targets --locked`
- `git diff --check`
- `cargo audit --no-fetch`
- `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`
- release-policy `check`, `boundary-check`, `reference-check`

## 8. Reporting

`.git-exclude/review-request/`. Include:

- **the error/absence split as implemented**, and whether `show_operation` stayed infallible;
- **how you corrupted an object for control 1** — that fixture is worth keeping and worth reusing;
- **any error class you judged should degrade rather than propagate**, with the by-design cause that
  justifies it. §6b names none, but you have been right about this surface twice now;
- **the base commit, stated exactly.** Round 2's §9 said `0a52411`; it was `9ba4117`.
