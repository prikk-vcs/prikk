# RFC 155 — The repository-complete artifact: a whole repository, verbatim, in one file

**Status.** **ACCEPTED by the project owner 2026-09-16** (*"RFC 155 is accepted."*); proposed the same
day by the architect. This is the "own RFC" that RFC 114 §5.2 (format carry-forward), RFC 115 §6.2 and
RFC 116 §7 left open, and RFC 154 §6.2 needs it. planeter's requirements letter
(`.git-exclude/upstream/planeter/receive/2026-09-16-repository-complete-artifact-requirements.md`, R1–R6)
is folded in as input. Author-review independence: the architect proposed and will review; §8's
controls compensate, and each must be shown to fail.

**The architect's reading of the approval, stated so it can be corrected:**
- **§7.1:** the direction is §3–§6 as written.
- **§7.2:** maintainer trust material travels as inert data and is never adopted by import.
- **§7.3:** landing goes through RFC 154's rule, and only on an explicit `--adopt`; the default stays
  `remotes/`, and DC-78 §D4's letter holds.
- **§7.4:** scheduling was not ruled by the word. The architect's proposal stands, after 0.43.0:
  1. the maintainer key-id collision fix;
  2. this artifact;
  3. RFC 154's adoption act.

  ROADMAP row 18 carries it. **Nothing is handed off yet.** R4's all-or-nothing import is the largest
  piece and gets a design round (report before implementation) of its own.

## 1. Why it is needed

- **Carry-forward.** RFC 114 §5.2 (owner-ruled) requires a supported, tested carry-forward before format
  7. A one-ref bundle is not one.
- **Keyless relay** (RFC 154). A holder with no key must be able to pass a repository's signed history on
  unchanged. Today a received ref cannot be re-exported (measured 2026-09-15).
- **Backup.** `bundle export` takes one ref. A restore today gives `remotes/<ref>`, not a working
  repository (`docs/src/guide/backup-restore.md:292`).

## 2. What exists, and what does not

- **`bundle export`/`verify`/`import`** (DC-78) cover **one ref**.
  - `verify` needs no repository and writes nothing, but checks structure only; no signature is
    verified (`backup-restore.md:161-162, 291`).
  - `import` records objects and author-key material, and creates a `remotes/` pointer. It advances no
    ref and adopts no maintainer key (`:292`).
- **`PEXCH002`** (sync) is a patch-level delta, deliberately not a repository (RFC 116 §1 (a)).
- **One envelope per id.** A repository refuses a second envelope for an id it already holds with
  different signature bytes, as an Integrity error (`decide_write_outcome`, `crates/prikk-store/src/foundation/index.rs:442-470`).
- **No repository-wide read snapshot exists** across many refs (RFC 115 §3, point 3).

## 3. Scope

The artifact is a file in, a file out, and off the network. It is not sync, not a protocol, and not a
backup service. RFC 115 §6.1's "protocol or transfer" is answered as **transfer** for this artifact only.
Negotiation stays RFC 116's.

## 4. Requirements and the architect's recommendation on each

**R1 — Complete.**
- The artifact carries **every stored object** and **every ref**: branches, closed branches, tags,
  received pointers, and the author-key container.
- Accepted-but-unsealed work (what `sync pending` lists) is stored objects: patches, blobs and recognition
  claims. It therefore travels. prikk carries no review state; what makes a claim an *open change* is
  the forge's own metadata.
- **The maintainer trust policy never travels as authority.** RFC 115 §6.2 names it "the dangerous one":
  carrying adopted keys into a receiver lets a sender widen what the receiver trusts. The adopted keys'
  **public material** travels as an inert, labeled record. `import` never adopts from it; an operator
  may adopt from it by a separate, explicit `trust maintainer add`. So planeter's "trust-policy entries"
  arrive as **material to decide on, not decisions**.

**R2 — Verifiable offline, before import.** `verify` on this artifact checks structure, closure and
**every signature against the key material the artifact itself carries**. It reports the result as
*internally consistent*, never *trusted*. Trust exists only against a receiver's own adopted set, at
import or after. A signature the carried material cannot check is reported, not skipped.

**R3 — Verbatim, never re-signed.** Every envelope travels byte-identical. This is RFC 154's accepted
principle and needs no new ruling.

**R4 — Read-only export; all-or-nothing import.**
- **Export** takes one repository-wide consistent view under the repository lock and writes nothing in
  the repository. This is the mechanism RFC 115 §3 found missing.
- **Import** is all-or-nothing, **including when the process is killed**: a target is either untouched
  or complete. It is never partial, and `doctor` names any interrupted state. This is the hard part: a
  multi-ref import is WAL-scale work, not a loop over `bundle import`.

**R5 — Driven as a subprocess.**
- Every verb has a `--format json` outcome with stable fields.
- Cancelling means killing the process, which R4 makes safe.
- Bounded in memory by streaming, not by loading the whole artifact. The bound is measured, not
  asserted.

**R6 — Identity-stable, idempotent.**
- An object already held with identical bytes is a no-op. Re-importing the same artifact changes nothing
  and says so.
- The same id with **different** signature bytes stays **refused and named**; never union and never
  pick (one envelope per id). Verbatim relay (R3) never creates that case. Two holders who signed the
  same content separately do.

## 5. Where a complete import lands (RFC 115 §6.3)

RFC 115 §6.3 asks how one operation can serve both a repository moving itself and foreign history. By
default, `import` lands every ref under `remotes/`, as today. DC-78 §D4's letter holds: import creates
no ref the operator did not ask for.

With an explicit `--adopt`, the operator asks. For each ref, import then applies RFC 154's adoption rule
against the **target's own** adopted set:
- a ref whose chain is signed by an adopted key, and is a fast-forward (from nothing, in a fresh
  repository), **lands as a local ref**;
- every other ref still lands under `remotes/`, and the outcome names why.

Adoption runs inside the same all-or-nothing import (R4), not as a second pass that could stop between
refs.

**Carry-forward** therefore reads as follows. On the new version:
1. run `init`;
2. adopt the maintainer keys explicitly (from the artifact's inert record);
3. run `import --adopt`.

Every branch lands as a branch, no signature changes, and trust was never automatic.

## 6. Format

The artifact gets a new magic and version, `PREPO001`, under RFC 114's stability contract. The build that
introduces format 7 must read `PREPO001` written by the last format-6 build: that is the carry-forward
test. The layout's retired-format refusal messages then point at it.

## 7. What the owner rules

1. **Direction:** build this artifact as §3–§6 describe.
2. **The trust-policy rule of R1:** public material travels as inert data and is never adopted by
   import.
3. **Landing via RFC 154** (§5), rather than a separate "migration mode".
4. **Scheduling.** The architect proposes: after 0.43.0, following the maintainer key-id collision
   fix, before RFC 154's adoption act (§6.2 there).

## 8. Controls an implementation must show failing

- A byte flipped in any envelope makes `verify` fail and names the object.
- An artifact carrying a trust record: `import` leaves the target's adopted set unchanged.
- A process killed at each write stage of import leaves the target untouched or complete; `doctor` names
  the interrupted state.
- Export during a concurrent `seal` yields either the pre- or post-seal repository, never a mix.
- Re-importing the same artifact is a no-op, reported.
- The same id with different signature bytes is refused and named.
- A ref signed by a non-adopted key lands in `remotes/`, never as a branch.
- Round trip: export, import into a fresh repository, `verify`, and the ref set and object ids are
  identical.
