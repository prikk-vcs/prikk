# RFC 154 — A branch several maintainers advance: adopting a trusted, fast-forward ref advance

**Status.** **PROPOSED 2026-09-16 by the architect.** Arises from planeter's letter of 2026-09-15
(`.git-exclude/upstream/planeter/receive/2026-09-15-hosted-repository-trust-model-discussion.md`),
which asks whether a shared, multi-maintainer repository that holds no key is a case prikk intends to
support. That is a scope and trust-model decision, so it is the **owner's** (§8). Nothing here is
scheduled. Author-review independence: the architect proposes and would review; the gap is
compensated by §9's controls, each of which must be shown to fail.

## 1. The question

A forge is a shared point that several maintainers advance and everyone else reads. Today a
repository's branch advances only by a maintainer signature made **in that repository**, so a
keyless forge cannot advance or re-serve a branch at all (measured 2026-09-15). planeter names three
directions:

1. prikk lets a repository adopt a received, trusted-maintainer-signed ref advance as its own branch,
   keyless, advanceable by any of a set of trusted keys;
2. multi-maintainer hosting needs a forge signing key, and keyless is for single-maintainer projects;
3. a shape they have not seen.

## 2. What exists today

- **Trust is already a set.** A repository adopts maintainer keys by id; `required = 1` means any one
  adopted key's signature suffices, a constant (`docs/src/reference/repository-layout.md:248`).
  The same page says the store is not *"hosted forge policy, or a multi-maintainer threshold
  system"* (line 253). Key ids are trust-on-first-use (`docs/src/reference/trust-threat-model.md:164-167`).
- **A ref is already a linear, signed chain.** `RefStatePayload` carries `target_object_id`,
  `update_seq` and `previous_ref_state_id`; publication is a compare-and-swap on the expected
  previous state (`crates/prikk-store/src/refs/publication.rs:45-49, 202-203`).
- **Verification already accepts any adopted signer.** `verify_trusted_publication_envelope`
  (`crates/prikk-store/src/trust.rs:345`) passes an envelope carrying a signature by any adopted key,
  not by "this repository's" key; `verify` uses it (`verify/trust.rs:55`), and merge evidence applies
  it to imported content (`merge/evidence.rs:428-448`). A RefState signed elsewhere by an adopted key
  therefore already verifies. What is missing is a verb that **publishes** one.
- **Received refs are a separate, name-only index** (`crates/prikk-store/src/received.rs:1-14`):
  `remotes/<origin name>` → RefState id, never read by `verify`. The pointer index requires a
  RefState's embedded `ref_name` to equal the pointer's name, so a received `heads/main` state could
  only ever be published as `heads/main`.
- **Every write that publishes signs locally.** `verify_signer_trusted` gates seal, sync seal, merge,
  branch create/close, tag create and `sync adopt-tag` (`trust.rs:316` and its callers). RFC 117
  §6 (detail 2) reasoned that a receiver should sign its own tag, and `sync adopt-tag` does.

## 3. Rulings this would touch

- **DC-78 §D4**, *"Import never advances a local ref"*. Option 1 keeps the letter: import still advances
  nothing, and adoption is a separate, explicit act. It changes the premise behind D4.
- **The sync guide's principle** (`docs/src/guide/sync.md:69`): *"The receiver seals and adopts under
  its own key, always."* Option 1 amends it for one act. The receiver still decides, but it publishes a
  state a trusted key signed elsewhere, without re-signing it.
- **Repositories are anonymous** (settled). Option 1 does **not** reopen it: authority stays in signer
  keys, and nothing names a repository or an origin.
- **RFC 116 §7** lists remote-tracking semantics as a non-goal. Option 1 is not remote tracking: nothing
  follows an origin automatically.

## 4. The options

### Option 1 — fast-forward adoption of a trusted advance

`prikk` gains one explicit act. Given a RefState (with the chain segment, blocks and patches it
reaches), a repository publishes it as its own ref of the **same signed name** if and only if:

1. its signature is by a key in **this** repository's adopted set;
2. it is a **fast-forward**: its `previous_ref_state_id` chain reaches the local tip (or the local
   ref does not exist and the chain begins at its root), with `update_seq` strictly increasing;
3. every block it reaches passes the checks `verify` applies, and replay derives the state it names;
4. the local compare-and-swap on the expected previous state holds at publication.

Nothing is re-signed, so the chain has one identity wherever it goes. Anything else is refused and
reported, never picked: a non-fast-forward, an unknown signer, a name mismatch, or two trusted
successors of one state.

**How several maintainers share `main`.** The forge adopts maintainer A's advance. Maintainer B's
advance from the same prior state is then not a fast-forward and is refused. B adopts A's advance
locally, merges their own work on top (an ordinary local merge, now possible because `heads/main` is
published), seals, and sends. B's state names A's as its predecessor, and the forge adopts it. So
*whose `main`* has an answer: one linear chain, ordered by the compare-and-swap, every link signed by an
adopted key and checkable by anyone who holds it. The one-envelope-per-id limit stops mattering,
because the forge holds one chain, not several maintainers' lines.

The primitive is **symmetric**: maintainer→forge, forge→maintainer and maintainer→maintainer are
the same act. It is not a forge mode, and prikk learns nothing about hosting.

### Option 2 — a forge signing key

The forge holds a maintainer key and seals or merges what maintainers send, re-signing as itself. That
works on today's machinery, apart from the key-id collision (§6.3).

### Option 3 — the shapes considered

- **Each receiver re-seals** (today's `sync seal`). Ids differ per repository, so no two parties share a
  chain, and a keyless forge cannot do it. This is the problem, not an answer.
- **A threshold of maintainers** per advance. Stronger against one stolen key, but it is quorum work the
  owner ruled out for v0 (DC-35 / RFC 152), and it still needs option 1's adoption act underneath.
- **Option 1 as a primitive, not a mode.** This is the third shape, and it is what §5 recommends: nothing
  forge-specific, only the missing inverse of publication.

## 5. Security comparison

| Threat | Option 1 | Option 2 |
|---|---|---|
| Forge fully compromised | Cannot create or alter history anyone accepts: it holds no key, and every receiver checks signatures against **its own** adopted set. It can withhold, serve a stale tip, or show different clients different trusted successors (split view). | Holds a trusted key, so it can sign any history every client that adopted the forge key will accept. The key is online by necessity. |
| One maintainer key stolen | Attacker can advance `main` (`required = 1`), visibly, as that key. | Same, plus the forge key is a second, hotter target. |
| Rollback to an older tip | Refused: not a fast-forward. | Refused by the same compare-and-swap. |
| Attribution | The publication signer is the maintainer who advanced. | Collapses to the forge; only patch authorship survives. |
| Key removed later | Needs a ruling (§6.5). | Same question for the forge key. |

What option 1 does not cover, and prikk should say so plainly:
- **Freshness** (a withheld or stale tip) is transport's to detect. It belongs to planeter.
- **Split view** is detectable only when two holders compare. prikk can make the evidence first-class
  by reporting two trusted successors of one state as a refusal that names both.

A forge's adopted set is an **admission filter** for what it serves. It is never a trust anchor for
anyone else; each client verifies against its own set.

## 6. What option 1 requires

1. **The owner's rulings of §8.1–§8.2** (the premise behind D4, and the sync-guide principle).
2. **An artifact carrying a ref's chain segment**: the RefStates (and RefUpdates, if the log
   consistency check requires them; to be established in design, not assumed), with the blocks and
   patches they reach. This relates to the repository-complete artifact owed under RFC 114 §5.2, which
   remains its own RFC; the design must decide whether one artifact serves both.
3. **The key-id collision fixed first.** `setup` names every maintainer key `maintainer`, and ids are
   trust-on-first-use, so two maintainers made by `setup` cannot both be adopted (ROADMAP §C, measured
   2026-09-15). A multi-maintainer primitive on colliding ids is unusable.
4. **Worktree-write trust.** An adopted tip becomes a worktree-writing anchor only through RFC 136
   §10.3c's replay-verified record, never on arrival.
5. **Key removal.** Does a state signed before its key was removed still verify, and may it still be
   adopted? Today removal means the key stops counting. A design must rule rather than inherit.
6. **Closed refs** (`closed` in the payload): adopting a close must be the same act, not a bypass.
7. **Relation to DC-85** (merge from a received ref): adoption is the fast-forward case, and DC-85 stays
   the diverged case.

## 7. Non-goals

Hosting, networking, accounts, fetch scheduling, remote tracking, repository identity, and threshold
signatures. prikk stays bytes-in, bytes-out.

## 8. What the owner rules

1. **Direction.** Option 1 as a symmetric primitive (recommended), option 2 as the intended pattern
   for shared hosting, or neither for now.
2. **If option 1:** amending the premise of DC-78 §D4 and the sync guide's "under its own key, always"
   for this one act.
3. **Scheduling**, relative to 0.43 (RFC 136), RFC 153 (`diff`), the key-id collision fix and the
   repository-complete artifact.

**Architect's recommendation: option 1.** Its failure mode under a compromised forge is denial
(withhold, stale, split view), not forgery. Option 2's is forgery. The fast-forward rule also answers
"whose `main`" with machinery prikk already verifies. The honest cost is §6: two prerequisites
(collision, artifact) and two rulings that change stated principles.

## 9. Controls an implementation must show failing

- An adoption signed by a non-adopted key is refused; perturb the key and see the refusal.
- A non-fast-forward (sibling successor, older tip, lower `update_seq`) is refused and names both states.
- A name mismatch is refused.
- A block that fails replay is refused, with nothing published and no worktree written.
- Two concurrent adoptions against one tip: exactly one publishes, and the other is refused on the
  compare-and-swap.
- `verify` passes on a repository whose `heads/main` holds states by two different adopted keys.
