# RFC 144 increment 3, piece 4 — the honesty invariant, enforced structurally

**Design:** `rfcs/accepted/144-two-point-comparison.md` **§4o.6**, implementing **§4i.1**'s ruling.
**The last piece of increment 3.** Follows `9a1b7dc2`, `cabe2487`, `25c45734`, `0e04164a`, `88e61bdd`.

## 1. The obligation, verbatim

> **Any surface that presents a `RenamePath` must keep the asserting signer recoverable *in the same
> answer*. The shared object carries the evidence (the signature); the belief is local; the two must
> never be separated at the point of display.**

**Why it matters, in §4i.1's own terms:** a signed `RenamePath(a→b)` asserts *"signer S asserts a→b"*,
which is **true** whether or not anyone believes S. The one way this system could produce a lying object
is **a read surface rendering a rename as a bare fact while suppressing who asserted it** — laundering a
claim into a truth. The invariant forbids exactly that and nothing else.

**And §4i.1 rules it must be a property, not a rule:** *"the rename and its asserting signer should be
one value in the read types, not two fields a surface may render separately."* **If a rename-bearing
value cannot be constructed without its signer, no future surface can drop it.**

## 2. CORRECTED — the surface list. §4o.6 named the wrong set, in both directions

I wrote that this "changes read types across `show`, `bundle preview` and `checkout --patch-plan`."
**Checked before writing this handoff, and that is wrong twice over.**

**In scope — these present a `RenamePath`:**

| Surface | Where |
|---|---|
| `show` | `show.rs:281`, `ShowOperationContent::RenamePath` |
| `merge-evidence` | rename operands now reach classification (`0e04164a`, `88e61bdd`) |
| **`status` / `worktree-status`** | `worktree_status.rs:420`, `QueuedOperationEntry { kind: "rename-path" }` — **I never named this one** |

**Out of scope, by construction — verify this rather than trusting me:**

- **`bundle preview`** has no `Renamed` effect variant, deliberately (§4m). A rename surfaces as
  delete+create effects, which **under-claims rather than launders** — the failure §4i.1 names is
  claiming a rename without its signer, not declining to claim one. `rename_note` already says so.
- **`checkout --patch-plan --format json`** fails the whole call on a `RenamePath`. It never presents one.

**If your own check disagrees with either row, report it before building** — the surface list is the
scope, and I have already got it wrong once.

## 3. RULED — carry the AUTHOR key id, and it is not optional

**The asserting signer is the patch's AUTHOR signature key id** — §4h ruled provenance rides in the
signature, not the operation, so it is not a field on `RenamePath`.

**It is always recoverable, so the type must not make it optional.** `verify.rs:352-354`'s own doc
settles this: a missing AUTHOR signature is *"propagated as an `Err` the same way every other
authorship-integrity defect in this pipeline is, **not a value this type carries**."* An `Option` here
would model a state the pipeline refuses to produce, and every `Option` is an invitation to render the
`None` case as a bare rename — **which is the exact failure this piece exists to prevent.**

**Carry the key id and nothing else. Do not carry trust, verification status, or any belief.** §4i.1:
*"the belief is local"*, and §4h.3: **trust policy must never be an input to replay**. `Sound` versus
`Unverifiable` is a separate, local question with its own existing type; conflating it here would put a
trust judgement inside a shared read value.

## 4. RULED — the enforcement shape

**Put the key id inside the rename variant**, not beside it as a sibling field on the report. A sibling
field is *"two fields a surface may render separately"*, which §4i.1 explicitly rejects.

**The test is whether a rename-bearing value can be constructed at all without a signer.** State in your
report **how the compiler enforces it** — and demonstrate it: attempt a construction that omits the
signer and quote the compiler's refusal. A test cannot prove the absence of a back door; the type
signature can.

**Naming matters here more than usual.** These are public read types and the field will outlive every one
of us on this project. Prefer a name that says *who asserted this*, not one that says *signature* — the
value is an assertion's author, not a cryptographic artefact.

## 5. Controls

1. **Each of §2's three surfaces presents a rename with its asserting key id** — through the real path,
   not a constructed value, and including `--format json` where the surface has one.
2. **The two out-of-scope surfaces still do not present a rename as a rename** — `bundle preview` still
   reports delete+create effects, `checkout --patch-plan --format json` still fails the call. **These are
   regression guards on a deliberate absence**, which is the kind most easily lost.
3. **A patch whose AUTHOR signature is absent is an `Err`**, not a rendered rename with a blank signer —
   asserting the pipeline behaviour §3 rests on, so a later change cannot quietly make it representable.

**Perturb control 1.** It is the one whose passing is most easily vacuous: a surface that renders the
signer *sometimes* would still pass a test that only checks the happy path. Break one surface's signer
plumbing and confirm its control fails and the others do not.

## 6. What this round must NOT do

- **No trust or verification status in the read types.** Not now, not as a convenience field.
- **Do not add a `Renamed` variant to `bundle preview`** — §4m ruled its absence deliberate, and adding
  one here would smuggle a design change into an invariant round.
- Do not touch `patch_replay`, the seal path, `rename_nodes_checked_batch`, the declaration store, the
  disclosure lines, the move hint, or `0e04164a`/`88e61bdd`'s classification and ordering.
- No new witness kinds. No resolution mechanism.
- `MILESTONES.md` untouched.

## 7. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9. **State rule 9's cross-target result, or that
it does not apply and why.**

**These are public types on a published crate** — `CHANGELOG.md` earns a line, and the shape change is
user-visible in `--format json` on at least `show` and `status`. Check whether the docs for those
surfaces enumerate the rename fields; if they do, they change too.

Report to `.git-exclude/review-request/`. **Lead with §4's demonstration** — the compiler refusing a
construction without the signer is the deliverable; the three surfaces are its consequence.
