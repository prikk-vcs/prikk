# RFC 145 — Serving a repository for reading

**Status.** **PROPOSED 2026-09-10.** Opened by the architect on the project owner's direction of
2026-09-06, scheduled in `ROADMAP.md` as *"Open the hosting-shape RFC — `prikk instaweb` vs. a separate
small server"* and deliberately not ranked first (owner: *"No need to take it as the most
prioritized."*).

**Its gate opened when RFC 142 shipped the content surface in 0.36.0.** RFC 142 §1 recorded the
dependency in the other direction — *"the hosting-shape RFC is gated on this. A `prikk instaweb`-style
browse view can name changed paths and show no line of what changed"* — and that gate is now open.
Nothing else blocks the design.

**This RFC does not decide to build anything.** It rules **which shape** a read-only browse view should
take, on measured cost, and names the decisions that are the owner's rather than the architect's. §8
carries the recommendation; §9 carries what is the owner's call.

**Author-review independence gap:** the architect authored this and will review its implementation.
Recorded per standing practice; compensated by §7's controls being stated before any code exists.

---

## 1. What was asked, and what has already been ruled

**The owner's words, recorded 2026-09-06 and not to be re-derived:**

> *"Will be happy if we have either of prikk subcommand like git's `git instaweb` or another project of
> a small hosting server."*

**Already ruled, and this RFC does not reopen either:**

- **Full hosting — GitLab/Forgejo-shaped — is not available**, on five structural grounds: no network
  transport at all, no ref authorization model, repositories are anonymous by settled design, multi-user
  concurrency is undesigned (RFC 108 §D5), and the format is explicitly unstable in public.
- **Four of those five do not apply to the read-only shape**, corrected the same day. `git instaweb`
  accepts no pushes, has no accounts, and addresses no remote, so ref authorization, repository
  identity, multi-user concurrency and long-term format stability all fall away.

**What remains from that list, and it is the one that still applies: prikk has no network code, by
position and not by omission.** RFC 116 §1 states it as a design commitment — *"`prikk-store` stays
bytes-in/bytes-out and prikk stays off the network"* — and §4 builds on it: *"ship sync over any channel
a person already has, with no network code at all."*

**This RFC's central question is therefore not "what should a browse view show?" but "does a browse view
require prikk to acquire network code, and what does that cost?"**

## 2. There are three shapes, not two — and `git instaweb` is the third

The scheduling entry names two. **The comparison is missing the one the owner's own example actually
is.**

**`git instaweb` does not contain a web server.** Verified against `git instaweb --help`, which
describes it as *"a simple script to set up gitweb and a web server"* and lists the daemons it can drive
— *"apache2, lighttpd, mongoose, plackup, python and webrick"*, defaulting to lighttpd. **Git ships the
*view* (`gitweb`, a CGI script) and borrows the *serving*.** Stated with its source because it changes
the choice materially and should not be taken on trust.

So the real candidates are:

| | shape | who listens on a socket | new prikk dependencies |
|---|---|---|---|
| **A** | **`prikk instaweb`** — a subcommand that serves over HTTP | **the binary that writes repositories** | an HTTP server, or hand-rolled HTTP |
| **B** | **A separate small server project** — its own crate, linking `prikk-store` | a separate binary | none in prikk; whatever that project chooses |
| **C** | **Static export** — `prikk` writes a directory of HTML/JSON; any existing server serves it | **nobody, inside prikk** | **none** |

**C is what `git instaweb` is closest to in spirit** — prikk ships the view, the machine supplies the
serving — and it is the only one of the three that costs prikk nothing at all.

## 3. The content surface exists now, and this is what a view could already show

Measured against the shipped 0.37.0 command inventory rather than assumed:

- **`prikk show <block-id|patch-id> [--format json]`** — what a block or patch changed, **including
  content** (RFC 142, shipped 0.36.0).
- **`prikk checkout --patch-plan --format json --content-path <path>`** — replayed content for named
  paths (RFC 143, shipped 0.37.0).
- **`prikk log`**, **`prikk branch`**, **`prikk tag`** — history, branches, tags.
- **`prikk verify --format json`**, **`prikk status --format json`**, **`prikk trust maintainer list
  --format json`** — trust and integrity state.
- **`prikk bundle preview --format json`**, **`prikk worktree-status --format json`**.

**So the wall the stikk project reported — a view that can name changed paths but not show a line of
what changed — is gone.** A browse view built today can show content.

**`prikk diff` remains refused** (RFC 143): prikk has no comparison semantics, and the consumer takes
the comparison. A browse view is a consumer and may render one; it must not claim prikk computed it.

## 4. The cost that decides this: prikk's runtime dependency surface is five crates

**Measured at the workspace root, 2026-09-10.** Every third-party runtime dependency prikk has:

`ed25519-dalek`, `getrandom`, `rustix`, `sha2`, `windows-sys`. **Five.** No serialization framework, no
argument parser, no async runtime, no HTTP anything. `cargo audit` resolves 216 packages across the
whole graph including dev-dependencies and tooling; the direct runtime surface is five.

**This is the project's most unusual property and its cheapest security posture.** Every dependency is
code the project has not read, shipping in a binary that signs and writes user data. Five is auditable.

**What each shape does to it:**

- **A, with a server crate.** `axum`/`hyper` brings an async runtime and tens of transitive packages.
  **This would be the largest single change to prikk's threat surface in the project's history**, and it
  would arrive to serve a read-only convenience view.
- **A, hand-rolled.** A read-only HTTP/1.1 responder over `std::net::TcpListener` adds **zero**
  dependencies and is genuinely feasible at this scope. **But it means prikk maintains an HTTP parser** —
  and an HTTP parser in the binary that writes repositories is a security asset we would own forever, for
  a browse view.
- **B.** **Zero new dependencies in prikk.** The separate project picks its own and carries its own risk.
- **C.** **Zero.** No parser, no socket, no listener.

**This is the decisive axis, and it is not close.**

## 5. What a listening socket costs, stated plainly

The owner's standing constraint is that *security is very important and should be protected as strongly
as possible and reasonable.* Shape A puts a listening socket **in the same binary that holds signing
keys' code paths, acquires repository locks, and writes the WAL.** Concretely:

- **Bind address is a security decision, not a convenience one.** `127.0.0.1` only, never `0.0.0.0`, and
  it must not be configurable into a wider bind without the user typing something explicit. **Note the
  precedent to refuse:** `git instaweb` makes localhost-only an *opt-in* flag (`--local`, *"only bind the
  web server to the local IP"*), so its default binds wider. **prikk should invert that** — localhost is
  the only behaviour, and anything else is a separate decision the owner takes deliberately.
- **Path handling becomes attacker-facing.** Every request path is arbitrary text that must not become a
  filesystem path. prikk already has a path-safety conformance body of work (DC-72) built for exactly
  this class, but it was built for *artifact* input, not for a request line.
- **Resource exhaustion becomes reachable.** A browse view walks history and materializes content; RFC
  133 measured commit memory as **O(nodes)**, linear past ~4,000 nodes at ~1,781 B/node. A request that
  names a large ref is a memory amplifier, and today nothing bounds it because nothing needed to.
- **The repository content is served in the clear.** The sync surface already warns of this for
  artifacts; a browse view would need the same warning and would be easier to leave running.

**None of these is unanswerable.** They are listed because shape A's true cost is this list plus the
parser, not "a subcommand."

**Shapes B and C move every item on this list out of prikk.** In C, nothing binds a socket at all.

## 6. The library route is open — measured, not assumed

Shape B is only viable if a separate project can read a repository **without** shelling out to the CLI
and scraping text. It can:

`prikk-store`'s curated public API already exports `load_ref_history`, `RefHistory`, `HistoryEntry`,
`load_received_ref_history`, `show`, `ShowPatch`, `ShowOperation`, `ShowBlobContent`,
`prepare_merge_evidence`, `SnapshotManifest`, `compute_state_root` and the trust surface. **History and
content are both reachable as typed values.** All eight crates publish to crates.io, so an external
project can depend on `prikk-store` by version.

**This matters more than it looks.** It means shape B needs **no new prikk feature at all** to begin, and
that a JSON gap in the CLI (§6a) constrains only the shell-out route.

### 6a. The JSON gap is real, but it binds only shapes that shell out

**`log`, `branch` and `tag` have no `--format json`** — verified against the 0.37.0 inventory. Those are
exactly the three an index page needs first. Nine commands have machine-readable output; those three do
not.

**Under shape B or a library-backed A, this gap does not block anything**, because the typed API is
already there. **Under a shell-out implementation it is the first blocker.** Recording it so that a
future round does not discover it as a surprise, and so that it is not mistaken for a prerequisite of
the whole RFC.

**It is worth closing on its own merits regardless of this RFC** — `--format json` on the three
read-only listing commands is small, independently useful, and consistent with the nine that have it.
That is a candidate increment, not a dependency.

## 7. What any shape must not do

- **No writes.** A browse view is read-only. It must not acquire a write lock, must not append to the
  WAL, and must not seal, publish or repair anything. **Serving must not be able to mutate a
  repository**, and this should be structural, not a review promise.
- **No claim that prikk computed a comparison.** RFC 143 refused `prikk diff`; a rendered comparison is
  the view's, and must be labelled as the view's.
- **No new format.** A browse view consumes existing surfaces. It does not mint a schema number, and it
  is not a reason to add one.
- **No repository identity.** Repositories are anonymous by settled design. A view must not invent a
  name, an owner or a URL identity for one.
- **No unbounded response.** Whatever bounds §5 needs, they are part of the first increment, not a
  follow-up — a view that works on a small repository and exhausts memory on a large one is worse than
  no view.

## 8. Recommendation

**HELD FOR EXTERNAL REVIEW 2026-09-10, on the project owner's instruction** — *"I want to discuss and
consider very carefully on it, the advancement, before start."* Review request drafted as
`external-arch/send/draft/009-…`. **Nothing below is to be implemented until that discussion closes.**

### 8a. The prior art runs against this recommendation, and it is recorded before the recommendation

**Git is the outlier, not the rule.** Most comparable systems ship the serving:

- **Fossil** — `fossil ui` / `fossil server`, a built-in HTTP server in the same single binary that
  writes the repository, in a project that is deliberately self-contained and low-dependency. **That is
  §5's exact objection embodied by the project whose posture most resembles the one §4 is protecting.**
- **Mercurial** — `hg serve`, built in.
- **Git** — borrows an external httpd, which is what shape C models.

**So the design this RFC calls wrong is what most comparable systems chose.** No principled reason for
the split has been found, and one is not invented here. **The most likely candidate, unmeasured and
stated as a suspicion rather than a finding: a static export must decide in advance what to
materialize, and a large history makes that either enormous or lossy** — which is precisely what an
on-demand server avoids.

**These are claims about external projects, not measurements taken here**, and the review request asks
the external architect to confirm or correct them. **The recommendation below is held at lower
confidence than the rest of this RFC because of this section.**



**Recommended: C first, then B if demand survives it. Not A.**

**Why C first.** It costs prikk nothing on the axis that matters — zero dependencies, zero network code,
no socket, no parser, no bind address, no request-path handling. It honours RFC 116 §1 exactly rather
than carving an exception into it. Its output is a directory a person can inspect, copy, publish to
object storage, or serve from a web server they already run — **which is precisely what RFC 116 §4 says
prikk should do with every other artifact it produces** (*"four blobs move over anything"*). And it is
reversible: a static exporter that proves nobody wants it costs a directory of dead code, not a
permanent HTTP surface.

**Why B second and not first.** B is the right long-term home for anything interactive, and it needs no
prikk change to start — but it is a second project to maintain, and opening one before knowing whether
the view is wanted is the more expensive experiment.

**Why not A.** Shape A's *hand-rolled* variant is technically feasible at zero dependency cost, and that
is the version worth naming rather than dismissing. **The objection is not feasibility, it is
ownership**: it puts an HTTP parser and a listening socket permanently inside the binary that writes
repositories and holds signing paths, to serve a convenience. **Five runtime dependencies is a posture
the project has paid for.** Spending it on a browse view is the wrong trade, and the same view is
available under C at no cost.

**If the owner prefers A regardless — which is their call — then it should be the hand-rolled variant,
never a server framework**, and §5's list becomes the increment's requirement set rather than its risk
register.

## 9. Decisions that are the owner's

0. **Whether the external review changes the recommendation.** Held open by the owner's own
   instruction; §8a is the reason it may.
1. **Shape.** C, B, A, or none. The architect recommends **C**, with B held for later — **at reduced
   confidence, per §8a**.
2. **Whether this is scheduled at all, and against what.** It is currently ranked second and the owner
   has already said it need not be first. **It competes with nothing urgent**, and the honest position is
   that no adopter has asked for it — the stikk project asked for a content surface, which shipped.
3. **Whether the `--format json` gap on `log`/`branch`/`tag` (§6a) is worth closing now**, independently
   of the shape decision. The architect thinks yes, on consistency grounds, and it is small.

### 9a. The name is not `instaweb`, and the owner is right

**Owner, 2026-09-10: *"the name of `instaweb` seems unfamiliar with prikk."*** Recorded because it is
correct and because the reason generalizes.

prikk's command vocabulary is plain and literal throughout — `init`, `commit`, `status`, `seal`,
`branch`, `tag`, `bundle`, `log`, `checkout`, `show`, `verify`, `doctor`, `unlock`, `compact`, `sync`,
`mv`. **There is not one portmanteau or coinage in it.** `instaweb` is both, and it is *borrowed* —
which asserts that prikk's browse view is git's browse view.

**RULED: `instaweb` is not the name, whatever the shape.** The actual name follows the shape and is not
settled here: under C the thing being named is an **export**, not a server, and `instaweb` would be
actively misleading for it. **`instaweb` is used in this RFC only as the owner's shorthand for the
question**, never as a proposed command name.

## 10. Non-goals

- **Not hosting.** No pushes, no accounts, no ACLs, no remote addressing. That question was answered and
  is not reopened here.
- **Not a transport.** This RFC adds nothing to RFC 116's sync path and must not become a way to move
  artifacts between machines.
- **Not a UI design.** What the view *shows* is a later question; this RFC rules where it lives and what
  it may cost.
- **Not a `prikk diff`.** Refused in RFC 143 and still refused.

## 11. Revisit triggers

- **An adopter asks for it in writing.** Today none has; the stikk project's asks were content-surface
  asks and shipped.
- **A network transport is built for another reason.** If RFC 116's optional transport is ever taken, the
  cost calculus in §4 changes and A becomes cheaper than it is today.
- **The dependency posture changes.** If prikk ever takes a serialization or async dependency for an
  unrelated reason, §4's decisive axis loses its force and this RFC should be re-argued rather than
  cited.
