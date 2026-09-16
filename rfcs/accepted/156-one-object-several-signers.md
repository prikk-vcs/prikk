# RFC 156 — One object, several signers

**Status.** **ACCEPTED by the project owner 2026-09-16**, proposed the same day by the architect as item 0 of 0.45.0
("working together"). Direction B (§4): *"Yes. Authorized."* §7.3's revised bound — a limit of **4** counted
signatures per object, the exemption based on previously recorded keys removed — confirmed: *"Reviewed. Authorized."*

**The architect's reading of §7.3 rule 1, stated so it can be corrected:** "signatures made by this repository's own
configured signing keys" is implemented as **signatures written by this repository's local writers** (`commit`,
`seal`, `merge`, `sync seal`, `rollback-draft`, `tag create`, `sync adopt-tag`) — never counted, never refused — while
every AUTHOR signature arriving through `bundle import` or `sync accept` counts. An import never has to read secret key
material to recognise a local key, and no outsider can reach a local writer, so the property is the same.
Handoff: `rfcs/handoffs/156-one-object-several-signers/one-object-several-signers-handoff-v1.md`.

Author-review independence: the architect proposes and would review; §9's
controls compensate, each of which must be shown to fail.

## 1. The question

A prikk object's id does not cover its signatures, and a repository stores **one envelope per id**: the first copy it
stores is kept, and a later copy of the same object carrying different signatures is refused. So **the order in which
copies arrive decides whose signature a repository keeps.**

That has two consequences, one of them already public:

- **The limitation named in advisory GHSA-px5q-233r-6hq5**: history imported from a source you do not trust can be
  recorded under a signer you did not expect, and keep a later copy signed by the original author from being stored.
- **Ordinary collaboration meets the same refusal.** A receiver that seals the same patches in the same grouping onto
  the same parent produces the sender's block id; importing the sender's bundle then refuses with `existing container
  record for <id> differs from candidate` (measured 2026-09-15; stated in `docs/src/guide/sync.md`).

RFC 154 (adopting a signed chain verbatim) and RFC 155 R6 ("the same id with different signature bytes stays refused")
both assume the current answer. **This RFC asks what the answer should be**: what a repository does when one object
arrives under a second valid signature.

## 2. What exists, at source

- **The envelope already holds a signature list.** `ObjectEnvelope.signatures: Vec<Signature>`, "signatures over the
  object ID … not part of the object ID preimage" (`crates/prikk-object/src/envelope.rs:29-30`). They are kept in
  canonical order with duplicates refused (`validate_strict`), and `add_signature` inserts one without changing the id
  (`:119-136`). **Several signers on one object is already representable.**
- **The store refuses it by policy, not by structure.** `decide_write_outcome` refuses a same-id write with different
  envelope bytes as `Integrity` (`crates/prikk-store/src/foundation/index.rs:442-470`). The index lookup is already
  **last-entry-wins** (`lookup_object_location` searches in reverse, `:382-398`), and the rebuild keeps container order
  (`rebuild_index_from_containers`). A later record for the same id would be what every reader sees.
- **Authorship is "the first AUTHOR signature in canonical order."** `require_author_key_id` returns the first
  (`crates/prikk-store/src/author/author_signing.rs:72-85`), and **`verify` checks only that one**
  (`verify_author_signature_against_material`, `author_key_index.rs:489-541`). Any further AUTHOR signature on an envelope is neither verified nor reported today.
- **MAINTAINER trust is already multi-signature.** `verify_trusted_publication_envelope` accepts an envelope when any
  of its signatures is by an adopted key (`crates/prikk-store/src/trust.rs:345`).

## 3. Options

- **A — keep the first, refuse the rest (today).** Order decides attribution, and an earlier copy can block a later
  honest one. **Rejected**: this is the limitation itself.
- **B — a signature union.** A stored object's signature set is the union of every valid signature that has arrived for
  it. A copy adding nothing new is a no-op; a copy adding signatures writes a superseding record carrying the union.
  The result is the same whatever order the copies arrive in. **Recommended (§4).**
- **C — keep the first envelope, record further signatures beside it.** A second store of signatures, with two sources
  of truth for one object's signers, a new durable container, and a new transport section. Everything B achieves, at
  more format surface.
- **D — make signatures part of the id.** The same patch would get a different id per signer, which breaks content
  addressing, lineage, patch-set digests, merge confluence and every recorded id. **Rejected.**

## 4. Recommended: B, and the rules that make it safe

1. **Every signature stored is verified first**, against the key it claims, before the union is written. An invalid
   signature is refused and named, never stored.
2. **A MAINTAINER signature is stored only if an adopted key made it.** Trust comes only from adopted keys, so a
   non-adopted maintainer signature adds nothing a reader can act on. It is dropped and reported, not stored.
3. **An AUTHOR signature is stored if it verifies against that key id's recorded material** (trust-on-first-use,
   unchanged). The key-id binding rules stay exactly as they are.
4. **The union is written as one superseding record** — canonical order, idempotent, order-independent. No record is
   rewritten; the container stays append-only.
5. **`verify` checks every signature of every role**, not the first AUTHOR signature only (§2's latent gap closes
   whatever else is decided).
6. **Authorship is a set, and nothing picks one silently.** `show` and `log` report every AUTHOR signer. In JSON a new
   `author_key_ids` list is added. The existing `author_key_id` keeps its current meaning (the first in canonical
   order), documented as such, so no consumer's parse changes.
7. **Refusals stay before writes** (0.44.0's rule): an import or exchange that would store an invalid signature, or
   exceed §7's bound, writes nothing.

**What B does to the two consequences in §1:** a copy from an untrusted source can add a **visible** extra signer, but
it can no longer remove, replace or block an honest one. The collaboration case stops refusing: the receiver's own
seal and the sender's copy merge into one block carrying both maintainer signatures.

## 5. Compatibility — a gate, not an assumption

**An older binary (0.44.0) must still open, read and verify a repository holding superseding records.** Its lookup is
already last-entry-wins, and its trust check already accepts any adopted signature. But its AUTHOR check reads the
first signature only, and whether its container scan tolerates two records for one id must be **measured, not read**.

- **If 0.44.0 verifies such a repository:** B ships as a minor release.
- **If it does not:** B is a repository-format change. Format 7 then requires RFC 114 §5.2's carry-forward first, which
  means RFC 155 lands before this RFC — the schedule changes, and the architect reports it.

**Transport:** bundles and exchange artifacts already carry whole envelopes, signature lists included, so the union
travels without a format change. An older receiver meeting a union envelope refuses it exactly as it refuses a
different copy today; a newer receiver merges it. Both behaviours are measured and stated.

### 5a. MEASURED 2026-09-16 (Stage 0) — the gate fails for Blocks

Against the released 0.44.0 asset (reproduced by the architect):
- **a second record for a Block id** — a union, or byte-identical — makes `verify` fail its objects stage
  (`format-2 topological pass detected an inconsistent cycle count`; `block_state.rs:668` compares a set of ids with
  a count of records), and `doctor` exits 1;
- **a second record for a Patch id** passes every command;
- **a merged rename is misattributed**: 0.44.0 reports the first AUTHOR signature in canonical order, not the
  committer;
- **0.44.0 refuses a repository marked format 7 at open**, with every command exiting 1, though its message says
  `unsupported format version: 0`.

**By this section's rule, B as specified is a repository-format change.** The choice between a superset format 7
reached by an explicit in-place upgrade, a Patch/Block split, or waiting for RFC 155 goes to the owner — together
with whether an in-place upgrade satisfies RFC 114 §5.2. Review: `.git-exclude/reviewed/one-object-several-signers-stage0-review-v1.md`.

## 6. What this changes elsewhere

- **RFC 155 R6** becomes "an id already held is merged under §4's rules, never silently replaced".
- **RFC 154's adoption** of a chain containing an object the repository already holds merges it, instead of refusing.
- **`sync.md`** loses the refusal it documents, and **the advisory's related limitation** is marked closed in the
  release that ships this.

## 7. Bounding the signer set — revised 2026-09-16, confirmed by the owner

**The first recommendation (8, with no limit for keys recorded before the import) rested on judgement, not measurement,
and its exemption does not hold.** Answering "why 8" produced the basis below.

### 7.1 What a bound controls, and what it cannot

- **It bounds cost, not blocking.** Anyone can mint AUTHOR keys, and each verifies under trust-on-first-use, so an
  adversary can fill a limit of *any* size. The choice of number buys nothing against blocking; it only limits how much
  one object can cost.
- **What one stored signature costs, at source.** Five canonical fields, each with an 11-byte header, plus its record
  header; the key id is at most 128 bytes (`SIGNATURE_KEY_ID_MAX_LEN`). That is **about 160 bytes typically, 274 at
  most**, plus **one Ed25519 verification each time `verify` runs**. An object filled to a limit of *N* costs up to
  *N* × 274 bytes more, and *N* + 1 verifications instead of one.
- **What legitimate use needs.** prikk signs a patch once, with its author's key. Nothing in the codebase, docs or RFCs
  co-signs a patch, and relay never re-signs (RFC 154/155). The plausible cases are the author plus a second key of
  their own, and rarely a byte-identical patch reached independently — **one to three signers**. **There is no
  observed data**: the store has never allowed more than one, and this RFC says so rather than invent a figure.

### 7.2 The exemption was gameable

"No limit on keys this repository had already recorded before the import" does not bound anything. Keys are recorded
**by imports**, which anyone can supply: one import records many keys on unrelated objects, and a later import signs a
target object with all of them, now "already recorded". The protected class must be one the operator controls, not one
imports can create.

### 7.3 Revised rule

1. **Never counted, never refused:** signatures made by **this repository's own configured signing keys**, and
   MAINTAINER signatures by **adopted keys** (rule 2 already stores no other maintainer signature). The operator
   controls both sets.
2. **Every other AUTHOR signature counts against one limit per object, across all imports** — a total, not a per-import
   allowance.
3. **The limit is 4**: the original author plus room for three more, above the one-to-three legitimate range with
   margin. Worst case per object: about 1.1 KB more and five verifications instead of one. At 8 it would be 2.2 KB and
   nine, for no gain against blocking.
4. **At the limit, the next counted signature is refused and named** — the object, and that its set is full — with
   nothing written, so an operator can see a filled set rather than meet a silent omission.
5. **The limit is one named constant, documented.** Raising it later is compatible: releases before this RFC store no
   union at all, so no older reader enforces it. The trigger to raise it is a legitimate workflow shown to need more
   than four.

**Residual, stated in the docs:** a signer that neither this repository's operator configured nor adopted can be
refused on an object whose set other such signers have already filled.

## 8. Non-goals

Threshold signatures or quorum trust (ruled out for v0, RFC 152); key revocation or rotation; any change to how a key
id binds to a public key; network transport.

## 9. Controls an implementation must show failing

1. **Order independence.** Two copies of one object under different valid signatures, imported in both orders, give
   byte-identical stored envelopes. Perturb by keeping the first.
2. **An honest copy after a foreign one** is stored, and both signers are reported.
3. **An invalid signature** is refused and nothing is written.
4. **A non-adopted MAINTAINER signature** is not stored and is reported.
5. **`verify` fails on an envelope whose second AUTHOR signature is invalid.** Perturb by checking the first only.
6. **The collaboration case:** the receiver seals, then imports the sender's bundle — merged, `verify` clean, both
   maintainer signatures reported.
7. **§5's gate:** a 0.44.0 binary opens and verifies a repository holding a superseding record — or the RFC's schedule
   changes.
8. **The bound (§7.3):** at four counted signers the next is refused and named with nothing written; a signature by
   this repository's own configured key, or by an adopted maintainer key, is still added; and keys recorded by an
   earlier import still count (§7.2's exemption must not come back).
