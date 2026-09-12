# RFC 138 — Asking what a repository trusts

**Status.** **CLOSED 2026-09-06 and moved to `rfcs/done/` — shipped in 0.34.0** (`f1be4c4`).
`prikk trust maintainer list` and `prikk trust maintainer check --key-id <ID>` are released, both with
`--format json`. **All six of §7's controls hold in the shipped binary**, including the one that was
the point of the RFC: `check` exits `0` whichever way the question resolves, so a negative answer is an
answer and not an operational failure (RFC 121).

**What outlives this RFC, and it is not this RFC's work.** The implementing round found that
`read_current_trust_policy_snapshot` (`prikk-store/src/trust_index.rs`) returns `Ok(None)` for two
genuinely different states — a policy never configured, and a policy whose *very first* record is torn
short of its own 50-byte header. **A pre-existing ambiguity `seal` already inherits**, narrow (only
reachable from a crash during the first-ever `trust maintainer add`) and practically self-correcting,
but named here because this is the file a future reader of that code will reach for.

Everything below is the record as it stood. Previously: **ACCEPTED by the project owner 2026-09-06**, the same day it was opened.

**Moved to `rfcs/accepted/` 2026-09-06, on the owner's reminder that RFC-000's 5-folder variant is
adopted.** The trigger is **design complete**, not handoff issued — `accepted/` means *"review
complete; implementer may start"*, and §7.4 said an implementer may start. The architect had said "it
moves when I issue the handoff", which is one step too late. **Third correction of this kind; §9.4a of
RFC 120 proposes the gate that would catch it.**

**What the acceptance covers, stated because a bare acceptance is scope-ambiguous.** It accepts the
problem record and the option space. **§4 was left unruled and did not need the owner** — §7 rules it
all: **both surfaces**, `list` and `check`, with **`--format json` riding along**, because that is what
keeps `check` inside RFC 121's exit-code contract rather than filing a negative answer as an
operational failure. **Nothing in this RFC now awaits the owner.** **Handoff issued:** `rfcs/handoffs/138-trust-policy-read-surface/trust-read-surface-handoff-v1.md`.

Originally opened as: **PROPOSED, 2026-09-06.** Opened at the project owner's instruction after the stikk
project asked for it (`.git-exclude/upstream/stikk/receive/002-trust-listing-and-the-no-audit-flag.md`),
and the owner approved the architect's recommendation that it is worth doing.

**Author-review independence.** The architect wrote this and is its only reviewer — the standing gap
on every architect-authored design here.

**Tracks.** A read surface over state prikk already holds. **No change to what is trusted, how trust
is adopted, or what trust means.**

---

## 1. The question nobody can ask

`prikk trust maintainer` offers `add` and `remove` (`commands.rs:115-116`). **There is no way to ask
which keys a repository currently trusts.**

This bites a caller that must answer *"can I actually seal here?"* before starting a ceremony that
ends in immutable history. `seal` requires a MAINTAINER key adopted **in the repository's trust
policy**, not merely present in the environment — so seeing the environment is not seeing the answer.

**`prikk verify` looks like it answers this and does not.** It prints `sealed-block <id>: <key_id>`
per sealed block (DC-78 Stage 2 §D3). Two reasons that is not the same question:

- **It is historical signer attribution, not current policy.** A key that signed a block may have been
  revoked since; the line still prints.
- **It does not exist before the first seal.** An unsealed repository prints nothing — and that is
  exactly when the question is asked.

**So the gap is worst precisely where a caller meets it.**

## 2. The capability exists; only the surface is missing

`load_maintainer_trust_policy(layout) -> MaintainerTrustPolicy` (`trust.rs:214`) returns
`keys: Vec<AdoptedMaintainerKey>` — *"every currently adopted key, in the order it was adopted"* —
where each carries `key_id: String` and `public_key: [u8; 32]` (`trust.rs:54-59`).

**This RFC adds no read, no state and no policy. It exposes one that is already loaded on every
seal.**

## 3. What "trusted" means here — and one thing the CLI already prints that is not read

Two facts a read surface must not blur, both from `trust.rs:61-64`:

1. **It is object trust, not ref authority.** *"A `Block`/`RefState` is trusted if **any** adopted key
   signed it… adopting a key never lets it move a ref — `RefStore::publish` still requires a signature
   from this operator's own signer."* A listing answers "whose signatures does this repository accept
   on objects", **not** "who may publish here".
2. **There is no threshold.** `MaintainerTrustPolicy` holds a `Vec` and nothing else. Trust is
   any-of-N by construction.

**Found while writing this: `policy: required=1` is a hard-coded literal**, printed by
`main.rs:295` and `setup.rs:106`. It is *true* — any one adopted key suffices — but it is **printed as
if read**, and no such field exists. A read surface that reported it the same way would be inventing a
policy value. **Either derive it or stop printing it as policy**; this RFC must not add a third site
that repeats a constant in the voice of a query.

## 4. The option space

**(a) `prikk trust maintainer list`** — enumerate adopted keys. What the request asked for. Cheapest
to consume; commits us to enumerating a trust policy as a public surface.

**(b) `prikk trust maintainer check --key-id <ID>`** — answer one question, exit-coded. **The
requester offered this as a fallback and it may be the better shape**: it matches
`verify_signer_trusted`'s own question, it is what a ceremony actually asks, and it commits us to
nothing about enumeration. Its cost is that a caller wanting the whole picture must already know the
ids.

**(c) both.** They answer different questions and (b) is not a restricted (a) — one is *"what is
here"*, the other *"is this here"*.

**(d) machine-readable output, as a first-class question rather than a follow-up.** `verify --format
json` is the existing precedent. **The requester's highest-value standing ask across our whole CLI is
a machine-readable surface**, and adding a human-only listing now would create a second thing to
retrofit. Deciding it here costs a paragraph; deciding it later costs a format.

## 5. Threat model — checked, not assumed

**Nothing here is secret.** Adopted public keys were typed on the operator's own command line by
`trust maintainer add --public-key HEX`. A listing returns what that operator put there.

**No tension with repository anonymity** (settled: no repository identity, no peers, only artifacts).
That property is about repositories not *carrying* identity; it says nothing about an operator reading
their own local policy. The requester checked `trust-threat-model.md` before asking and read it
correctly.

**One thing to keep straight in any output**: §3.1's distinction. A caller who reads "trusted" as "may
publish here" has been misled by us, not by themselves.

## 7. §4 RULED by the architect, 2026-09-06

**The owner accepted this RFC without answering §4, and §4 does not need them.** RFC 135 §9.8 is the
precedent: a question the architect has the material to settle is not a question to submit. All of §4
is settled here.

### 7.1 Both surfaces — (c)

**`prikk trust maintainer list`** and **`prikk trust maintainer check --key-id <ID>`**. They answer
different questions and neither is a restricted form of the other: one is *"what is here"*, the other
*"is this here"*. A ceremony asks the second; someone who inherited a repository, or ran `setup` weeks
ago, asks the first.

**§4(a)'s worry — that listing "commits us to enumerating a trust policy as a public surface" —
evaporates on §5.** There is nothing to protect: every adopted public key was typed on the operator's
own command line. **Withholding a listing would be a restriction with no beneficiary**, and the
requester raised the worry as a guess about our model, not as a position of theirs.

**Cost: one loader, two presentations.** `load_maintainer_trust_policy` already returns exactly what
both need.

### 7.2 `--format json` rides along — (d) — and it is not a nicety

**It is what keeps §7.1's `check` inside RFC 121's ruled exit-code contract**, which is the part that
would otherwise go wrong.

RFC 121 ruled the vocabulary: **`0` ok · `1` operational failure — findings, integrity failure,
refusal · `2` usage error.** *"key X is not trusted"* is **none of those**. The command was asked a
question and answered it; nothing failed and nothing was refused. **Exiting `1` for a negative answer
would file a successful query as an operational failure — precisely the conflation the stikk project
reported to us in their first letter**, committed inside the command written to answer their second.

**So: `check` exits `0` whenever it determines the answer**, and carries the answer in its output.
`1` and `2` keep their ruled meanings — an unreadable policy, a bad argument.

**And that is exactly why machine-readable output cannot be a follow-up.** Without it, a caller who
must branch on the answer has only prose to parse, and the pressure to overload the exit code becomes
real. `verify --format json` is the existing precedent, so this follows a shape rather than inventing
one.

**This settles the format for these two commands and nothing else.** The general machine-readable
error surface — RFC 132's amendment records it as the requester's highest-value ask across the whole
CLI — remains an unopened design question, and one command adopting an existing flag does not answer
it.

### 7.3 What the output must and must not say

- **Key id and public key**, plus adoption order, which `MaintainerTrustPolicy`'s `Vec` already
  carries. Nothing here is secret (§5).
- **It must not say "required=1" or otherwise report a threshold as policy.** §3's finding stands:
  that literal is printed at two sites and read from nowhere. **This RFC adds no third site.**
- **It must not read as ref authority.** §3.1: adopting a key means prikk accepts its signatures on
  objects; it never lets that key move a ref. Wording that lets a caller conclude otherwise is a
  defect, not a nuance.

### 7.4 Status

**Nothing in this RFC awaits the owner.** §3's `required=1` defect is separate, predates this work, and
is named for whoever resolves it. A handoff may be written.

## 6. What this RFC does not decide

Which of §4 (a)/(b)/(c), and whether §4(d) rides with it. **§3's `required=1` finding is a defect to
resolve either way** — it predates this RFC and is not caused by it.

**Out of scope:** any change to adoption, revocation, thresholds, multi-maintainer policy, remote
trust, or what `seal` requires. **Key management and rotation remain unimplemented and unscheduled**
(`IMPLEMENTATION-STATUS.md`), and nothing here moves them.
