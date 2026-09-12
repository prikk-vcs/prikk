# RFC 135 — The entrance: what a new user meets before anything works

**Status.** **Proposed.** Raised by the project owner 2026-09-04, on reading the `README.md` Quick
Start: *"They are generally unfamiliar with visitors. I doubt it makes visitor feel uneasy and brings
their withdrawal."*

**CLOSED 2026-09-06 and moved to `rfcs/done/` — shipped in 0.33.0** (`19f5cea`). `prikk key
generate/public` and `prikk setup` are released; the measured entrance cost is **11 unfamiliar steps
to 5**, and the step that was *impossible* (deriving a public key from a seed) is now a command.

**What outlives this RFC, and it is not work:** §9.1's deferral of `prikk config` — **with §4's whole
dependency/config-format question deferred alongside it** — re-opens on one named trigger, **a first
real adopter**. This is the design record to reopen against, and §9.6's refusal of a credential-helper
boundary carries its own, different trigger (a user asking for one).

**One disclosed gap ships with it:** `--out`'s Windows refusal is compiled but executed by no CI job.
If DC-87's Windows mutation work lands, that is the untested branch to pick up.

**Folder corrected 2026-09-06: `proposed/` → `accepted/`.** RFC-000 makes folder location lifecycle
authority, and `accepted/` means *"review complete; implementer may start"* — which became true the
moment §9's design was accepted and a handoff was issued. **Caught by the project owner, and this is
the second time in two days** (123/130/131/137 were the first). §9.9 records what should have caught it
instead of a person.

**§9 DESIGNS IT, 2026-09-06**, on the owner's instruction after 0.32.0 shipped the landing page and
made the entrance live. **`prikk config` is deferred with a named trigger — which removes §4's
dependency question from the critical path entirely** — `prikk key`/`prikk setup` are ruled in, a
helper boundary is deliberately refused, and **§6 is answered: prikk stores no secret**, but may write
one to a path the user names, `ssh-keygen`-style. **§9.8's two questions were withdrawn and ruled by the architect the same day, on the owner's challenge that neither should have been asked**: `--out` is in (printing a seed to a terminal is the *less* secure option, and with `--out` the seed is written and never printed), and `setup` is a single command composed over a first-class sequence, which must show the trust decision it makes. **The design is complete; nothing on this RFC awaits the owner.** **Handoff issued:** `rfcs/handoffs/135-first-run-entrance-and-configuration/key-and-setup-handoff-v1.md`.

**Deliberately unhurried.** The owner asked for careful design *"now and later with time we need"*, and
directed that security be considered seriously. **This RFC opens the problem and the option space; it
settles nothing.** The architect's first framing — a small key command — was rejected by the owner as
*"insufficient"*, and rightly: it sized the fix to a symptom the architect had personally hit rather
than to what a visitor meets.

**Tracks.** The first-run surface, and the configuration mechanism prikk does not have.

---

## 1. What a visitor meets today

`README.md`'s Quick Start, before anything happens: **four `export` lines carrying two opaque 32-byte
hex seeds**, then a `trust maintainer add` carrying **a third opaque hex value**, then `commit` and
`seal`.

**The values are correct** — the architect derived the public key from the sample maintainer seed and
it matches the README byte for byte. **Nothing is broken.** The question is entirely what it costs a
reader to get past.

## 2. It is not reproducible with your own key — and that is a capability gap, not a documentation one

The three hex strings are **not independent**: `--public-key` must be the Ed25519 public key *of*
`PRIKK_MAINTAINER_SEED`. Substitute your own seed and the trust entry silently stops corresponding.

**And prikk offers no way to recompute it.** There are **23 CLI commands and not one generates or
derives a key.** `prikk_crypto::Ed25519KeyPair::generate()` exists; **no CLI path reaches it.**

**First-hand evidence rather than supposition:** preparing 0.31.0's cross-version test, the architect
needed a maintainer public key for a seed and had to write a throwaway Rust crate depending on
`prikk-crypto` by path to print one. **If that is the cheapest route available to the project's own
architect, a visitor has none.**

**So the Quick Start is take-it-or-leave-it**: copy all of it exactly, or supply your own values and
be unable to complete step three.

## 3. Twelve variables, three separable concerns

Every `PRIKK_*` variable the CLI reads:

| Kind | Variables |
|---|---|
| **Policy, non-secret** (8) | `ACTIVE_PATCH_WARN`, `ACTIVE_PATCH_LIMIT`, `BUNDLE_MAX_BYTES`, `BUNDLE_MAX_OBJECTS`, `EXCHANGE_MAX_BYTES`, `EXCHANGE_MAX_OBJECTS`, `SYNC_SUMMARY_MAX_BYTES`, `SYNC_SUMMARY_MAX_REFS` |
| **Identity, non-secret** (2) | `AUTHOR_KEY_ID`, `MAINTAINER_KEY_ID` |
| **Secret key material** (2) | `AUTHOR_SEED`, `MAINTAINER_SEED` |

**These are three problems, not one, and conflating them is what made the first framing too small.**

- **The eight policy values are an already-admitted gap.** `main.rs` says of the threshold pair:
  *"never persisted in the repository — a durable policy belongs to a future general configuration
  increment."* Today every non-default limit must be re-exported **in every shell and every CI job**,
  permanently. **This carries no security question at all.**
- **The two key ids are not secret** and are pure friction.
- **The two seeds are private key material**, and everything about persisting them is a security
  decision (§6).

## 4. The dependency question — a decision, not a prohibition

**Corrected 2026-09-04.** The architect first wrote that `placement.rs`'s `ALLOWED_THIRD_PARTY` entry
`("prikk", &[])` puts every settings crate *"out by construction"*. **The owner refused that reasoning
and was right.**

**The constant states no policy.** It is a per-crate allowlist in which every non-empty entry carries a
recorded reason — `windows-sys` cites DC-96 — and `("prikk", &[])` records **what the CLI currently
depends on, not what it may ever depend on.** It is the same allowlist-with-reasons idiom as
`UNSAFE_EXEMPT_CRATES`, whose own module doc states the principle: *a visible edit to a reviewed
constant is the control; invisibility was the problem.*

**So adding a dependency to the CLI is a reviewable act, not an impossibility** — the same act that
admitted `sha2`, `ed25519-dalek`, `getrandom`, `rustix`, and `windows-sys` to their crates.

**The owner's position, recorded because it governs this RFC's design:** *"monolith without
carefulness is not preferred. If some external crate is well designed and properly maintained, there
is (or is not) possibility to rely on it. Of course, we should be really careful and take sufficient
verification."*

**This project already has a method for exactly that question, and it should be used rather than
re-invented.** **DC-50** was an explicit return-on-investment decision on whether to depend on `sha2`
or write SHA-256 first-party; it produced a decision record, and **DC-55** implemented the outcome.
**DC-79** and **DC-80** are dependency *upgrades* carried out as their own reviewed increments. The
project has both adopted and displaced dependencies deliberately, with evidence each time.

**Corrected again, same day.** The architect then wrote that a `std`-only format *"has no
supply-chain surface, no version-skew risk, and **cannot half-parse**."* **The last clause is false**,
and the owner refused it: *"Independence is not always the safest. It is on the premise that there are
solid design and sufficient tests / verification. I care about ours is not config file parser."*

**A hand-rolled parser half-parses exactly as easily as any other** — more easily if it is written once
and lightly tested. `.prikkignore` fails closed **because it was designed and reviewed to**, not
because `std` parsed it.

**The honest accounting, then:**

| | `std`-only, ours | A crate |
|---|---|---|
| Supply-chain surface | none | real |
| Version skew | none | real |
| Correctness | **ours to design, test, and maintain, forever** | someone else's, and their maintenance is the thing being trusted |
| Competence | **not this project's domain** | their domain |

**This project's own history is the sharpest evidence, and it is against writing another parser.**
`AUD-03` records *"four-plus hand-copied TLV cursors; the duplicate-field strictness split in RFC 125
§2.3 exists because each copy evolved separately."* And RFC 125's own outcome records that **the class
was larger than the RFC said**: §2.3 named two files, implementation found seven plus a second decoder
inside one of them. **When this project writes its own parsers they multiply and drift**, and the
correction is still open work.

**So neither side is automatically safer, and the burden is symmetric**: a crate asks us to verify
someone else's maintenance; our own parser asks us to fund verification in a domain that is not
prikk's, to the standard prikk holds everything else to. **DC-50's method is how that gets decided, and
one of its inputs must be what our own parser would cost to verify properly** — DC-50's answer for
SHA-256 came with frozen vectors and a 10,000-case differential, which is the price of doing it right.

**The precedent for the std-only shape**, if it wins:

**The precedent exists and is recent.** `.prikkignore` (RFC 124, `text_span`'s sibling `ignore.rs`) is
line-oriented, one directive per line, whitespace-trimmed, blank lines skipped, **fail-closed on
anything malformed**, and **absent means defaults**. Those four properties were reviewed and are the
right starting point for any config file here.

## 5. The option space

**None of these is recommended yet.** They are separable and can combine.

**(a) `prikk config` — durable, non-secret settings.** The eight policy values plus key *ids*. No
secret at rest, no lifecycle question, and it closes a gap `main.rs` already names. **The cheapest
real improvement, and the one with no security cost.**

**(b) `prikk key` — generate and derive.** Closes §2's capability gap: produce a fresh seed with its
public key, and derive a public key from a seed already held. Uses an API that exists. **Note it can
only print secret material to a terminal** while there is nowhere to put it — which is a reason to
consider (a) and (c) together rather than shipping (b) alone.

**(c) `prikk setup` — a flow, not storage.** Composes generation, trust registration, and telling the
user what to export. **It need not require prikk to own any secret**: it could emit a file the user
sources, keeping convenience separate from persistence.

**(d) A helper boundary**, as git separates config from credential storage and ssh keeps keys in files
the tool never invents. **Worth naming even if refused** — refusing it deliberately is a different act
from never having considered it.

## 5a. Who this is for — answered by the owner 2026-09-04

The architect asked which pain the entrance primarily addresses: the visitor who withdraws, or the
adopted user re-exporting eight policy variables in every shell and CI job. **The owner's answer
closes the question:**

> **"Now, there is no user 'who has already adopted it'. prikk has never been in production use yet."**

**So the second constituency does not exist**, and §5's sequencing follows from that rather than from
preference:

- **The eight policy values have no current beneficiary.** Their only consumer today is this project's
  own testing. Building durable configuration for them now is **speculative work against a hypothetical
  user** — the same trade RFC 132 §5 refused for `source()`, and it should be refused here for the same
  reason and revisited by the same trigger: **a first real adopter.**
- **The visitor is the only real user of this surface**, so §5(b) `key` and §5(c) `setup` are where the
  value is, and the measure of success is **how few unfamiliar steps precede the first working
  repository.**

**And the causation may run the other way, which sharpens it.** There are no adopters *and* the
entrance is four opaque hex exports before anything happens. **This RFC should not assume those facts
are unrelated.**

**One consequence for what "success" means here.** With adoption not yet the goal, the entrance's job
is to get a reader to *understand what is different* — which is the same job
`docs/src/reference/git-mapping.md` (RFC 128 §5) was just built for. **The entrance and that page are
one surface, and should be designed as one**, not as a CLI change beside a docs page.

## 6. Security — the part that must not be rushed

**Persisting a seed means private key material at rest**, owned by prikk, on a path prikk chooses,
under permissions prikk sets. Every one of those is a decision this project has not made.

**And it presses on a gap that is already open.** prikk has **no key rotation and no revocation beyond
the trust store**; `SECURITY.md` states plainly that release-signer verification of a `prikk` binary is
not yet available, and key lifecycle is deferred to RFC-025. **Making a long-lived key easy to keep is
not obviously an improvement while the machinery to retire one does not exist.**

**The honest framing**: convenience that increases the population of long-lived keys raises the cost of
not having lifecycle. **That trade is the owner's**, and this RFC must not smuggle it in as a
by-product of a nicer first run.

**A shape that avoids the trade entirely** is (c)'s emit-a-file form: prikk never stores a secret,
never chooses a path, never sets a permission — it hands the user something and says what it is.
**Less convenient, and materially smaller in security surface.**

## 7. Out of scope

**Key lifecycle** — rotation, revocation, hardware or threshold signing. RFC-025's, and named here only
because §6's decision changes its urgency.

**The two-key model itself.** Author and maintainer are separate deliberately: authoring is not
publishing. **This RFC does not propose collapsing them** — but it may propose that a visitor's first
success not require both, since `init` → `commit` → `verify` works today with the author key alone
(verified), and the maintainer ceremony is needed only to `seal`.

## 9. The design — §8's three open items, answered 2026-09-06

**Written on the owner's instruction after 0.32.0, when the landing page made the entrance live: a
visitor now arrives at a front door and meets the hallway this RFC is about.** §5a's ruling (the
visitor is the only real constituency) is settled input.

**Author-review independence:** the architect wrote this and is its only reviewer, the standing gap on
every architect-authored design here.

### 9.1 What is ruled in, what is deferred, and one question that disappears

| §5 option | Ruling |
|---|---|
| **(a) `prikk config`** — durable policy settings | **DEFERRED**, with a named trigger |
| **(b) `prikk key`** — generate and derive | **IN** (§9.3) |
| **(c) `prikk setup`** — a flow owning no secret | **IN** (§9.3) |
| **(d) a helper boundary** | **REFUSED deliberately** (§9.6) |

**(a) is deferred on §5a's own evidence, not on cost.** The eight policy values have no beneficiary —
their only consumer today is this project's testing. Building durable configuration for them now is
speculative work against a hypothetical user: **the same trade RFC 132 §5 refused for `source()`, and
it re-opens on the same trigger — a first real adopter.**

**The consequence is the most valuable thing in this section: §4's dependency question disappears with
it.** No config file means no format, no parser, no `serde`-or-hand-rolled decision, and none of the
supply-chain reasoning §4 opened. **The largest open question in this RFC leaves the critical path by
being unnecessary rather than by being answered.** It returns, intact and already argued, when (a)
does.

#### 9.1a RECORDED 2026-09-12 — what (a) would hold today, and the format candidate that must not be forgotten

**The project owner asked whether the format-crate candidate was planned anywhere it could be found.
It was not — it sat in RFC 148 §2b, an RFC about keys.** Recorded here, where the deferral lives, so
that whoever re-opens (a) from this section finds it.

**The trigger stands** — *a first real adopter* — and its reasoning still holds, checked against the
knobs that exist today rather than remembered:

| knob | today | after RFC 148 |
|---|---|---|
| `PRIKK_AUTHOR_KEY_ID` / `PRIKK_MAINTAINER_KEY_ID` | env, repeated per shell | **defaulted** to `author` / `maintainer`; env is an override |
| the two seeds | env, repeated per shell | **default directory, `0600` files**; no env channel |
| `PRIKK_ACTIVE_PATCH_LIMIT` (the threshold pair, §5a) | env override of `DEFAULT_ACTIVE_PATCH_LIMIT` (`main.rs:603-608`) | unchanged — **the one user-facing env knob that remains** |
| `--allow-no-audit` on every `seal` (21 docs mentions) | a flag | **deliberately not a default**: it is a per-seal trust decision and must stay loud |
| `--ref` on every command (14) | a flag | belongs to the `branch switch` / current-branch direction, not to config |

**So after RFC 148 lands, (a) would hold one knob.** That is not a beneficiary; it is the same picture
§9.1 deferred on. **(a) re-opens when a second per-user or per-repository setting appears that cannot
be defaulted, or when an adopter asks** — and both are now things a reader can check rather than infer.

**When it re-opens, §4's format question returns with these facts already gathered (2026-09-12):**
`app-json-settings` 2.7.0 — MSRV 1.85 (matches), normal dependencies `serde` + `serde_json`, optional
`windows` — provides a JSON settings file at a platform path with load/save. **It is a real candidate,
evaluated on merit per §4's own correction** (*a dependency is a decision, not a prohibition*), against
a hand-built reader. One benefit already visible either way: the CLI's tests carry three copies of a
hand-written JSON parser that `serde_json` would retire. **The decision is an `ALLOWED_THIRD_PARTY`
edit with a recorded reason, made then, not now.** Cross-reference: RFC 148 §2b; `ROADMAP.md`
"Remaining work" band D.

### 9.2 §6's question — does prikk ever store a secret? **No, with one named exception**

**The line: prikk never invents a location for secret material, never reads one back, and never manages
its lifecycle.** It may **write** a seed to a path the user explicitly names — once, mode `0600`,
refusing to overwrite, refusing any path inside `.prikk/`.

**That is the `ssh-keygen` model**, and it is chosen over a keystore for reasons that are permanent
rather than expedient. Owning key-at-rest means owning file permissions, encryption-or-not, rotation,
deletion, backup semantics, and Windows-versus-POSIX divergence — **a security surface this project
would carry forever**, in a product whose own posture is that it does not encrypt sync artifacts and
tells the user to *"move it only over a channel you trust"*. Inventing a keystore beside that would be
incoherent.

**Writing to a path the user names is categorically different from owning storage**: prikk does not
choose the location, does not read it back, and has no opinion about it afterwards. If the owner
judges even that too much, `key generate` without `--out` still closes §2's gap — the exception is
convenience, not mechanism.

### 9.3 The commands

**`prikk key generate [--out <path>]`** — draw 32 bytes from the OS CSPRNG, print the seed and its
public key, and print **exactly the `trust maintainer add` line and the export lines the user needs
next**. With `--out`, write the seed under §9.2's constraints.

**`prikk key public`** — derive a public key from a seed the user already holds.

> **The seed is read from an environment variable and never from argv. This is a ruling, not a
> preference.** On Linux `/proc/<pid>/cmdline` is world-readable, and a shell records argv in history;
> an environment variable is neither. A `--seed <hex>` flag would leak key material to every process
> on the machine, and it is exactly the flag a hurried implementer would add.

**`prikk setup`** — compose the above with `trust maintainer add` and emit the complete export block.
**It owns no secret and writes nothing unless `--out` is given.**

### 9.4 A correction to §5(b): *"uses an API that exists"* is half true

`Ed25519KeyPair::from_seed` and `::public_key_bytes` exist, so **`key public` is a thin wrapper over a
tested API.** But `Ed25519KeyPair::generate()` **discards the seed** — the struct holds only
`signing: SigningKey` and exposes no accessor (`prikk-crypto/src/lib.rs:41-72`). `key generate` needs a
seed-returning generator that does not exist yet.

**It belongs in `prikk-crypto`, not in the CLI.** Entropy handling stays in the crate that already owns
`getrandom` and already fails closed when the OS source is unavailable; and DC-51's placement gate
makes adding `getrandom` to `prikk-cli` a decision to justify rather than an import to write.

### 9.5 What success is, measured

§5a set the measure: **how few unfamiliar steps precede the first working repository.** Today, for a
sealed commit, eleven — of which the first three have no prikk support and **the third is impossible**:

1-2. obtain two 32-byte seeds (no tool) · **3. derive the maintainer public key — no command does
this** · 4-7. export four variables · 8. `init` · 9. `trust maintainer add` · 10. `commit` · 11. `seal`

**The evidence for step 3 is first-hand: while building a test repository during this RFC's own
session, the project architect obtained the maintainer public key by copying a hex literal out of
`.github/workflows/ci.yml`,** because nothing in the product derives one. That is what a visitor meets.

**After §9.3: `prikk setup` and the four exports it prints.** The unfamiliar-step count is the number
this design should be judged on, and it should be re-counted, not assumed, when the increment lands.

### 9.6 (d) refused, and the refusal recorded rather than skipped

§5(d) asked for a helper boundary — git's credential helpers, ssh's agent — to be *named even if
refused*, because refusing deliberately differs from never considering it.

**Refused.** A helper protocol is a second interface with its own compatibility surface, invented for a
product with no adopters and no evidence anyone wants to plug anything into it. §9.2's line already
gets the security benefit a helper would (prikk owns no secret at rest) at none of the cost. **Revisit
if a user asks for one**, which is a different trigger from (a)'s.

**External confirmation, 2026-09-06 — unsolicited, and the refusal was made without it.** The stikk
project (a TUI/GUI front-end driving prikk through the public CLI) declared the opposite boundary from
their side, unprompted: they *"will never invoke"* `prikk key generate` or `prikk key public`, hold a
**presence-only rule** on key material — checking that `PRIKK_*_SEED` is set and never reading, writing
or carrying its value — and enforce it with a test that walks their prikk-invoking module tree and
fails their build if either subcommand name appears. They point users at `setup`'s own output verbatim
instead. **The first real consumer of this entrance independently drew the boundary this section
refused to cross for them**, which is the strongest evidence available that (d)'s refusal was right,
and it arrived four days after the refusal was written. Recorded here because §9.6 was reasoned from
first principles with no consumer to check it against. Source:
`.git-exclude/external-communication/stikk/receive/003-standing-requests-content-and-enumeration.md`.

### 9.7 The entrance and `git-mapping.md` are one surface

§5a's closing point, carried into the design: with adoption not yet the goal, the entrance's job is to
get a reader to **understand what is different**, which is the job `docs/src/reference/git-mapping.md`
already has. **The docs half is not separate work and must not be scheduled as a follow-up** — a
`setup` flow that leaves a visitor correctly set up and still surprised has solved the smaller half.

### 9.8 Both questions withdrawn and ruled by the architect, 2026-09-06

**The owner asked why either was submitted. Neither should have been — both had an answer in the
material already in front of the architect, and asking moved a decision rather than making one.**

#### 9.8.1 `--out` is IN, because not having it is the less secure option

**The background the question omitted, and which decides it:** without `--out`, the only way to capture
a generated seed is to **read it off a terminal**. That puts secret key material into scrollback, into
`tmux`/`screen` buffers, into any terminal logging the user has enabled, and then invites them to move
it into an environment variable by typing or pasting an `export` line — **into shell history.**

**A direct write to a `0600` file is strictly safer than printing**, and the risk it is imagined to
introduce — a secret at rest — exists either way, because the user must put the seed somewhere to use
it across shells. The choice was never "secret at rest or not". It was **"secret at rest via a
controlled write, or via the least controlled channel available."**

**Ruled: `--out` is in, and it strengthens rather than weakens §9.2's line** — prikk still never
invents the location, never reads it back, never manages its lifecycle.

**With one refinement the question's framing had hidden:** *when `--out` is given, the seed is written
and **never printed**.* Printing it as well would reintroduce the exact scrollback exposure `--out`
exists to avoid. Output in that case is the public key and the next steps only.

#### 9.8.2 `setup` is a single command, composed over a first-class sequence

**"A documented sequence is defensible" was lazy and is withdrawn.** It fails this RFC's own success
measure: §9.5 defines success as **how few unfamiliar steps precede the first working repository**, and
a documented sequence reduces that count by **zero** — it only writes the steps down. Arguing for it
on implementation cost was doubly wrong; the owner has ruled that cost is not a factor here, and it
was not a good argument before that ruling either.

**The owner's own observation is the correct design and is stronger than the question:** *"even a
single command is actually based on sequence… sequence is essential and perhaps base."* So it is not
either/or —

- **The individual commands stay first-class and documented** — `key generate`, `trust maintainer add`,
  `init`. That is the sequence, it is what a reader follows to *understand*, and §9.7 makes
  understanding the entrance's actual job.
- **`prikk setup` composes them** for the person who wants a working repository now.

**One constraint that follows from taking security seriously, and does not weaken the command:**
`setup` **must show the trust decision it makes.** Registering a maintainer key is a trust act; a
one-shot flow that performs it invisibly teaches a user that trust registration is a formality. It
should print what it trusted and why that step exists — the composition may remove the *typing*, never
the *seeing*.

#### 9.8.3 Nothing remains for the owner on this RFC

§9.1's deferral, §9.2's line, §9.3's argv prohibition, §9.6's refusal, and now §9.8's two rulings are
all the architect's. **The design is complete and a handoff may be written.**

### 9.9 What should have caught the folder error, instead of a person

**This is the second lifecycle miss in two days**, and the owner caught both. A rule caught by a person
twice is a rule that should be mechanical.

**The invariant that would have caught both, stated exactly:** *an RFC in `rfcs/proposed/` must not
have a directory in `rfcs/handoffs/`.* A handoff is an instruction to an implementer to start; RFC-000
says an RFC an implementer may start on is `accepted/`, not `proposed/`. **The two cannot both be true,
and the check is a directory listing.**

Measured at `007caf6` before this move: **exactly one violation — this RFC.** After it, none. RFCs
123, 130, 131 and 137 would each have tripped it on the day their handoff was issued.

**It is narrower and cheaper than RFC 120 §9's widening**, which needs `rfcs/accepted/` drained first
(52 files, ~45 of them finished). This one needs nothing drained: it binds two directories that are
both already accurate, and it fires on the exact transition that has now been missed twice.

**Not implemented here.** It belongs in `boundary-check` beside `open_work_index` and `rfc_naming`, and
it is RFC 120's subject rather than this RFC's. **Recorded as a proposal for the owner**, who has an
open ruling on §9.4 of that RFC already.

## 8. What this RFC does not yet decide

The shape (§5), whether prikk ever stores a secret (§6), and the format of any config file (§4).

**What it does establish**: the gap in §2 is real and measured; the three concerns in §3 are
separable; and §4's dependency question is **open and decidable by DC-50's own method**, not closed by
the current allowlist.

**And two corrections worth carrying**, both from the owner, both on the same reflex:

1. **A mechanism read as a policy** — `candidate_sequence`'s structure as proof of reachability
   (RFC 134 §3), then this allowlist as a prohibition. **A control that makes a change visible is not
   a rule forbidding it.**
2. **Independence assumed to be safety.** *"Cannot half-parse"* was asserted of code this project would
   have to write, test and maintain. **Owning the code moves the risk; it does not remove it** — and
   `AUD-03` is this project's own standing evidence of where that lands.
