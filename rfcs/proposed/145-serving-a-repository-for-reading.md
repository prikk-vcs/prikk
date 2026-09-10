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

### 8a. SUPERSEDED by §8b — the prior art, sorted by tool, appeared to run against the recommendation

**WITHDRAWN 2026-09-10.** The sort was by tool; sorted by *where the browse server lives* the same evidence **supports** the ruling — see §8b. Kept because the counterweight was recorded before the recommendation and removing it would rewrite the reasoning's order.

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
### 8b. RESHAPED 2026-09-10 by external review — the axis was a proxy, and there is a fourth shape

**Reply received** (`external-arch/receive/audit-20260910-web/010-…`), reviewed at
`.git-exclude/reviewed/external-arch-010-hosting-shape-reply-review-v1.md`. **The conclusion — not A —
stands. Its foundation, its ordering, and the shape list all change.**

#### CORRECTED — §4's decisive axis was the wrong one

§4 made **prikk's five-crate dependency surface** decisive. **That count is a proxy, and it is exactly
what the prior art beats**: Fossil ships a built-in server at a low count, and §8 had already conceded
that a hand-rolled responder costs zero crates. **On the count, the argument loses.**

**The real axis:**

> **What capability does this add to the process that writes repositories and holds the signing paths,
> and does that capability belong there?**

Shape A's cost is **a listening socket and a permanent inbound attack surface in the binary with write
authority and key access** — incurred in full by a zero-dependency implementation. §4 stands as a
description of prikk's posture; **it is no longer the reason for the ruling.**

#### §7's "no writes, structurally" is answered, and it is the same axis

§7 required that a browse view be unable to write and left "how, structurally" open. **The answer is
not a runtime guard: do not link the write code at all.** A view built on a read-only facet that never
names `seal`, `publish`, the WAL or lock acquisition makes the refusal a property of what is compiled
in. **Shape A cannot have that property — it *is* the writing binary.**

#### CORRECTED — §8a's prior art supports the ruling once sorted by the right trait

§8a sorted by tool and concluded "git is the outlier." **Sorted by where the browse server lives:**

- **git's ecosystem is B/C** — `gitweb` (CGI), **`cgit`** (a separate program, dynamic over large
  history), **`stagit`** (a static generator — a working exemplar that C is viable). git ships none.
- **Mercurial is both, and its production shape is B** — `hg serve` is A; **`hgweb`** deploys as a WSGI
  application behind a web server, which is how hg is hosted at scale.
- **The patch-theory cousins all separated** — `darcsweb`, `darcsden`, Pijul's Nest. **None puts the
  server in the VCS binary**, and that is the family closest to prikk in model.
- **Fossil is the lone pure-A, and it is monolithic.**

**A tracks monolithic tools; B/C tracks modular ones and the patch-theory family.** prikk is both
modular and patch-theoretic.

**And the principled reason §8a declined to invent: Fossil had no B available** — no published library,
no separable read surface — so it put the server in-binary out of architectural constraint. **prikk can
buy the dynamic-server capability without paying A's cost, a trade Fossil could not make.** §8a's
counterweight is withdrawn.

#### NEW — shape D: ship plumbing, not a server

**Git's real lesson is not "borrow an httpd."** It is that git exposed **plumbing**, and every browse
view — gitweb, cgit, stagit — was built by others on it. **prikk already has the plumbing**: §6's
exported read surface *is* the browse substrate, today.

**Shape D: prikk commits to a stable-enough read library and ships no server at all.** §6's observation
that "shape B needs no new prikk feature to start" was half-seeing this — **that is not B, it is D
already existing.**

#### The reshaped recommendation

- **D is the posture.** prikk provides the read library as the browse substrate and keeps it stable —
  plumbing, not porcelain, which fits prikk's modular, low-surface identity best.
- **C is the right first *first-party* convenience** — a literal `export` of the log/index and
  per-block/per-patch views, **honest about its ceiling**: content-at-a-point over large history is
  O(paths × points) statically and does not export.
- **B is the intended answer for the dynamic/large-history case — not "held for later."** It is where
  cgit, hgweb and darcsden all landed.
- **A is out**, on the capability axis, not the crate count.

### 8c. CHALLENGED — the reply's own load-bearing assumption fails today, and narrowly

The reply names the assumption its argument rests on: that `prikk-store`'s read surface can be
presented as a clean read-only facet.

**Measured, it fails as stated.** `history.rs` imports `crate::refs::RefStore`, and **`RefStore::publish`
is `pub` on that same type** (`refs.rs:272`, `publish_with_object_store` at `:280`). A browse binary
calling `load_ref_history` links write authority. **"Provable by absence" is not available today.**

**It fails narrowly, on three findings:**

1. **Locks are already clean** — `refs.rs` references `RefLock` / `acquire_container_locks` /
   `ContainerLockGuard` **zero** times.
2. **`prikk-store` already carries a feature** (`test-support`), so the `#[cfg]` mechanism for a
   `read-only` facet is proven in this crate.
3. **A feature gate is not a crate split**, so RFC 130 §6's ruling against splitting `prikk-store` does
   not bind it.

**RECORDED, and it changes what "provable" means.** Rust links a crate's whole API; only dead-code
elimination strips uncalled code from the artifact. **Absence is provable by symbol inspection of the
built binary, not by construction — unless a `#[cfg]` gate removes it at compile time.** "Do not call
it" and "do not compile it" look identical in source and are not the same guarantee. **A `read-only`
feature is what would give the construction-level guarantee**, and it is the concrete next design
question if D or B is taken.


## 9. Decisions that are the owner's

0. **Whether the external review changes the recommendation.** Held open by the owner's own
   instruction; §8a is the reason it may.
1. **Shape.** **Four now, not three — §8b adds D.** The reshaped recommendation after external review:
   **D as the posture** (prikk ships the read library as the browse substrate and no server), **C as the
   first first-party convenience** (`export`, honest about its large-history ceiling), **B as the
   intended dynamic answer** rather than an afterthought, **A out** on the capability axis. §8a's
   reduced-confidence caveat is withdrawn — the corrected prior-art sort supports the ruling.
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
