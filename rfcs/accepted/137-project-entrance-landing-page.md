# RFC 137 — The project's entrance: a landing page, and how it stays true

**Status.** **ACCEPTED by the project owner 2026-09-04**, the same day it was opened at their
instruction, after a design discussion that settled the whole option space. **This RFC records decisions
already made rather than re-opening them** (§3), and contributes the two things the discussion did
not settle: **how a landing page stays true** (§4) and **what it may say that the other two entrance
surfaces do not** (§5).

**Folder corrected 2026-09-05: `proposed/` → `accepted/`.** Increments 1-4 are implemented and
shipped in 0.31.1; only increment 5 remains, blocked on DNS. **It is not `done/` yet** — RFC-000
reserves that for shipped-in-full. Caught by the project owner.

**What the acceptance covers, stated because a bare acceptance is scope-ambiguous.** Unlike RFC 136,
this RFC carries no open question, so acceptance **clears §7's increments 1-4 to be handed over**;
increment 5 waits on the domain and on the next publish. It also accepts §5's three-surface division
**including the maturity asymmetry the architect explicitly flagged for overrule** — that placing
"Current Status"/"Not a Good Fit Yet" in the landing page's first screen runs against the owner's own
guideline ruling that they are *secondary* in `README.md`. The owner did not overrule it, so **the
asymmetry stands as designed**: secondary in `README.md`, first-screen on the landing page.

**Handoffs issued:**
`rfcs/handoffs/137-project-entrance-landing-page/increment-1-html-code-context-handoff-v1.md`
(**delivered `3126a24`, accepted and pushed 2026-09-04**);
`rfcs/handoffs/137-project-entrance-landing-page/increments-2-3-site-url-and-staging-handoff-v1.md`
(**increment 2 delivered `ea687c9`, accepted and pushed 2026-09-05; increment 3 delivered `d3124e4`,
held unpushed and re-made in the increment-4 round — it fails without a landing page**);
`rfcs/handoffs/137-project-entrance-landing-page/increment-4-landing-page-handoff-v1.md`.

**Coupled to RFC 135 (first-run entrance and configuration), deliberately.** The landing page ends at
an install command; RFC 135 begins at what a new user meets once that command has run. They are two
halves of one arrival, and §8 states the seam.

**Author-review independence.** The architect wrote this RFC and is also its only reviewer — the
standing gap on every architect-authored design here. Compensated at implementation review.

**Tracks.** The published entrance. **No shipped-code behaviour change is proposed.**

---

## 1. The problem

**A visitor arriving at this project's web address is shown a table of contents.**
`docs/src/index.md` — 19 lines — opens with "Prikk Documentation", a paragraph of definition, a note
on the name's Norwegian etymology, and links. It is a good documentation index. It is not a front
door, and it is currently doing the job of one.

The owner raised this first about `README.md`, in a form that applies with more force to the web
entrance:

> Our "Quick Start" in `README.md` is not quick at all... They are generally unfamiliar with visitors.
> I doubt it makes visitor feel uneasy and brings their withdrawal.

`README.md` has since been slimmed 323 → 161 lines. The web entrance has not been touched, and it is
the surface a link from anywhere else in the world lands on.

## 2. Why this is its own RFC rather than a task

Three reasons, in order of weight:

1. **It creates a published surface with no gate.** Every other document this project publishes is
   Markdown, and 40 of them are mechanically checked against the live command registry. A landing page
   is HTML, and **no gate in this repository can read it** (§4). That is a design question, not a task.
2. **It changes what the project's URLs mean**, including one that is immutable (§6).
3. **It is a third statement of the same claims**, and this project has already ruled on duplication
   in a way that does not obviously extend to it (§5).

## 3. Decisions already made — recorded, not re-opened

Settled by the owner across the 2026-09-04 discussion
(`.git-exclude/reviewed/landing-page-hosting-shape-v1.md`,
`.git-exclude/reviewed/landing-page-owner-drafts-review-v1.md`):

| Decision | Value |
|---|---|
| Domain | `prikk.org` (owner acquiring) |
| Landing page URL | `/` |
| Documentation URL | `/docs/` — **subdirectory**, not `docs.prikk.org` |
| Repositories | **one**; one Pages site, one CNAME, one workflow |
| Landing page source | `docs/landing/` |
| mdBook root | `docs/` — **unchanged**; no rename |
| `build.build-dir` | unchanged — declined, with reasons recorded |
| Page content | the two 2026-09-04 drafts **merged**, not chosen between |
| Story image | used, **split into panels with its text as real HTML** |
| The 1.39 MB GIF | dropped |

**The single reason the subdirectory won**, restated because it is this RFC's own thesis: it keeps the
landing page inside the repository that gates everything else. A subdomain needs a second Pages site,
therefore a second repository, therefore a page with no CI, no `release-policy`, and no gate — the
surface most likely to carry a claim nobody re-checks.

## 4. How the landing page stays true — the design content of this RFC

### 4.1 The evidence that this is a real problem, not a theoretical one

Two landing-page drafts were written on 2026-09-04. Reviewed against the live command inventory, they
contained between them:

| Claim | Reality |
|---|---|
| "Give each session its own workspace" (draft-01 prose) | **No workspace concept exists.** 23 commands; none implements one |
| "Isolated Workspaces — Safe & Parallel" (draft-02 image) | Same. And `prikk --help` states in its own words: *"there is no `branch switch` yet, and no current-branch pointer"* |
| "Effortless Review — Clear Changes" (draft-02 image) | **No review command.** `merge-evidence`/`merge-plan` are read-only analysis surfaces |
| "Confident Merge" (draft-02 image) | Partial — `prikk merge` requires an explicit `--baseline-block ID` and seals only a proven-confluent merge |
| `https://github.com/nabbisen/prikk` ×3 (draft-01) | Two migrations stale; RFC 129 moved to `prikk-vcs/prikk` at `c69f5a9` |

**Three false claims and a stale URL, in two drafts written the same week, by two authors.** The
landing page is the surface where aspiration is most natural and least checked. That is the problem
this RFC exists to solve.

### 4.2 The gate cannot read HTML, and declaring the page anyway is worse than not declaring it

`code_regions` (`crates/prikk-cli/src/commands/tests.rs:128-166`) finds command mentions by scanning
for ``` fences and `` ` `` inline spans. Rule (A) then checks every `prikk <token>` in those regions
against the live `COMMANDS` registry.

**An HTML page has neither.** It writes `<code>prikk seal</code>`. Adding `docs/landing/index.html` to
`DECLARED_DOCUMENTS` today would make `code_regions` return zero regions, `command_tokens` return zero
tokens, and **rule (A) pass vacuously** — a gate that reads as coverage in the declaration list while
checking nothing, so the next reader sees it declared and stops looking.

`release-policy`'s `command_scan` does not help: it walks `.md`/`.yml`/`.yaml`/`.sh`, and `.html` is
not in that set.

### 4.3 Ruled: extend `code_regions` to recognise `<code>` and `<pre>`

**Accepted by the owner 2026-09-04**, over the two alternatives:

| Option | Why not |
|---|---|
| Generate the HTML from a declared `.md` | Needs a build step this project does not have; §19 of the owner's own direction argues against tooling for one page |
| Leave it ungated, check by hand at each release cut | §4.1 is what manual checking of this surface produces |

**It must land before the page does.** Same ordering argument DC-90 used for its own gate: a boundary
added afterwards documents what happened instead of constraining it.

**What this does and does not buy.** Rule (A) checks that every `prikk <command>` named on the page is
a real registry entry. It does **not** check prose claims — "workspace", "review", "parallel" name no
command and would pass. **The gate closes the command half of §4.1's table and none of the rest.**
Stating that plainly is part of the design: an unstated limit in a gate is how a vacuous assertion
gets believed.

The remaining half is covered by §5's rule and by the release-cut checklist, not by machinery.

## 5. Three entrance surfaces, and what each may say

After this change the project has three front doors:

| Surface | Reader | Arrives from |
|---|---|---|
| `docs/landing/index.html` at `/` | someone who has not decided to try prikk | a link, a search, crates.io `homepage` |
| `docs/src/index.md` at `/docs/` | someone who has decided, and wants to do something | the landing page, or a deep link |
| `README.md` | someone reading the code | GitHub, a fork, a clone |

**The owner's standing duplication ruling — *"Duplicate is allowed, because reader can access to docs
from each"* — permits overlap and does not require it.** It was made about documentation pages that
each need to stand alone. A landing page that restates `README.md` is not wrong by that ruling; it is
merely wasted, because its reader has not asked the question `README.md` answers.

**Proposed division, as a rule that can be applied rather than a preference:**

- **The landing page answers "what is this and why would I want it", and ends at one install
  command.** It states nothing a reader would need to verify, and makes no claim naming a capability
  (§4.1's failure mode). Its own direction document already says it: *"The landing page is not
  documentation."*
- **`/docs/` answers "how do I do the thing"** and remains the index it is.
- **`README.md` answers "what is this repository"** — crates, gates, current state — for a reader who
  is already inside it.

**The one claim the landing page must carry that the others need not:** what prikk is *not yet*. The
owner's guideline ruling placed "Current Status" and "Not a Good Fit Yet" as **secondary** in
`README.md`; on the landing page, a visitor deciding whether to try an early-implementation VCS needs
that in the first screen, not the footer. The architect's own draft carries it as a short
`.maturity` note under the hero. **This is a deliberate asymmetry between the three surfaces, not an
inconsistency.**

## 6. What the URL change costs — measured

Setting a custom domain makes GitHub redirect `prikk-vcs.github.io/prikk/<path>` to `<domain>/<path>`.

| Link | Count | Editable? |
|---|---:|---|
| book root | 6 in-repo, **plus `homepage` in every published crate version on crates.io** | in-repo yes; **published versions: never** |
| `reference/release-compatibility.html` | 2 | yes |
| `guide/ignore.html` | 1 | yes |

**Only three non-root deep links exist, all in files we control.** The one immutable link is the book
root, baked into `homepage = "https://prikk-vcs.github.io/prikk/"` (`Cargo.toml:33`) across eight
crates and every released version.

**The subdirectory layout improves that link rather than breaking it:** the redirect sends the book
root to `prikk.org` — the landing page. A visitor clicking "Homepage" on crates.io arrives at the
project's front door, which is what that field means. Under a subdomain they would arrive at a
documentation index instead.

**`homepage` should be updated to `https://prikk.org/` at the next publish.** Already-published
versions keep the old value and redirect correctly; nothing needs migrating.

## 7. Increments

Ordered. Each is independently reviewable; **1 gates 4.**

1. **Extend `code_regions` for `<code>`/`<pre>`** (§4.3). Must not change what it finds in the 40
   Markdown documents — that is the review's negative control.
2. **`book.toml` gains `site-url`.** **CORRECTED 2026-09-05, measured against mdBook 0.5.4 while
   writing the handoff:** the mechanism is a `<base>` tag in the generated 404 page, not absolute
   asset hrefs — asset references stay relative either way. Without `site-url` the page carries
   `<base href="/">`, so a 404 served at `/docs/guide/nope.html` resolves its CSS against the host
   root and renders unstyled. The effect this RFC described is real; the cause was stated wrongly.
   **And the value is host-dependent**: `/prikk/docs/` on today's `prikk-vcs.github.io/prikk/`
   deployment, `/docs/` after the domain — so this increment is **not** correct against the current
   deployment with the value this RFC originally named. One line, silent failure if skipped or set
   wrong.
3. **`docs.yml` gains a staging step** — landing page at the artifact root, built book under `docs/`,
   staged in the runner's temp directory so nothing new appears under `docs/`. Its `paths:` filter
   already covers `docs/**`, so `docs/landing/` needs no filter change. **BINDING ORDERING
   CONSTRAINT, found 2026-09-05:** landed alone this breaks the published site — it moves the book
   off the artifact root while no landing page exists, so `https://prikk-vcs.github.io/prikk/` would
   serve nothing. **Increment 3 must not reach `origin/main` before increment 4.** A placeholder page
   is not the answer: it would be publicly served as the project's front page, which is worse for a
   visitor than the documentation index there now.
4. **Build the page** at `docs/landing/`: the two drafts merged, the story image split into panels
   with HTML captions, the GIF dropped, the repository URL corrected, and the three false claims of
   §4.1 removed. Declare it in `DECLARED_DOCUMENTS`.
5. **`homepage` → `https://prikk.org/`** in `Cargo.toml`, **together with `book.toml`'s `site-url`
   moving from `/prikk/docs/` to `/docs/`** — one change, two files, neither correct without the
   other. **HARD-BLOCKED, measured 2026-09-05: `prikk.org` has no DNS record and does not resolve.**
   Neither half may land early: `site-url = "/docs/"` would break the 404 page's `<base>` while the
   book is still served under `/prikk/`, and `homepage` is baked permanently into whatever version
   publishes. **This does not hold up a release** — see §7.1.

## 7.1 Publishing before the domain exists is safe

**A release need not wait for `prikk.org`.** Setting a custom domain on a GitHub Pages site makes
`prikk-vcs.github.io/<repo>/<path>` redirect to `<domain>/<path>`, so a crate published today with
`homepage = "https://prikk-vcs.github.io/prikk/"` — permanent and unchangeable for that version —
**still resolves to `https://prikk.org/` after the cutover**, landing on the landing page, which is
what that field means. Increment 5 improves the value for versions published after it; it does not
rescue anything, and holding a cut for it buys nothing.

**Not blocked on the domain**, with one qualification found 2026-09-05: increments 1, 3 and 4 are
correct against the current `prikk-vcs.github.io/prikk/` deployment, and **increment 2 is correct there
only with the host-appropriate value** (`/prikk/docs/`, not `/docs/`). Increment 5 waits on the domain,
and **the `site-url` cutover rides with it** — the two are one change, recorded here so the second is
not forgotten.

## 7.2 CORRECTED 2026-09-08 — increment 5 is not "two files", and one of them is Rust

**§7's increment 5 says "one change, two files".** Swept at `a3333dd`: it is **four files plus one that
does not exist yet**, and the undercount matters because the missed one ships in every future release.

**Must change, together:**

| file | what |
|---|---|
| `Cargo.toml` | `homepage` — **baked permanently into each published version**, so it must land before a release, never during one |
| `docs/book.toml` | `site-url` `/prikk/docs/` → `/docs/` |
| `README.md` | **three** sites: the header badge, the crate-table badge, and the Documentation link |
| `tools/release-policy/src/release_notes.rs:52` | a hardcoded link to the release-compatibility reference **inside the notes body of every future release**. Rust, not documentation — **this is the one increment 5's own wording would have missed** |
| ~~`CNAME`~~ | **STRUCK 2026-09-08 — this was wrong.** Measured after the domain went live: **no `CNAME` file exists in the repo or the published artifact** (`https://prikk.org/CNAME` → 404) **and the site serves correctly at the apex regardless.** For an Actions-based Pages deploy the custom domain lives in repository settings, not in the artifact. **The requirement was asserted, not checked** — see §7.2c |

**Must NOT change — records of what was true:** `CHANGELOG.md` (past releases genuinely carried that
URL), `rfcs/done/129`, the RFC 137 handoffs, and ROADMAP's historical rows.

### 7.2c MEASURED 2026-09-08 — the domain is live, and three things it revealed

**`prikk.org` resolves to all four GitHub Pages addresses and serves both `/` and `/docs/`.** The old
host `prikk-vcs.github.io/prikk/` **301-redirects to `https://prikk.org/`**, so existing links survive
the move.

**Three findings, measured against the live site:**

1. **`www.prikk.org` has no TLS certificate.** `http://` returns a 301, but `https://www.prikk.org/`
   fails with *"no alternative certificate subject name matches target hostname"*. DNS is correct —
   `www` is a `CNAME` to the apex — so this is certificate provisioning, not configuration.
   **Owner-side: it may resolve itself within a day, and if it does not, the Pages custom-domain
   setting is where to look.** Nothing in this repository can fix it.

2. **`/docs/404.html` still carries `<base href="/prikk/docs/">`** — the stale `site-url`, confirmed
   live. **This is exactly what increment 5 fixes.**

3. **There is no site-root `/404.html`.** `docs.yml` stages the landing page at the artifact root and
   the book under `docs/`, and the landing page has no 404 of its own — so **any mistyped URL at
   `prikk.org` renders GitHub's generic "Page not found · GitHub Pages"**, not anything of ours.
   **Not in increment 5's scope as written, and worth its own decision** rather than being swept in.

**§7's own rationale for `site-url` needs a caveat too:** it argued that a 404 served under `/docs/`
would resolve its CSS against the host root and render unstyled. **GitHub Pages serves only the
site-root `404.html`**, so `/docs/404.html` is not actually served to anyone today. **Fixing
`site-url` remains correct** — the file should not carry a wrong base — **but the user-visible symptom
the RFC described does not currently occur**, and finding 3 is the reason.

### 7.2a ADDED 2026-09-08 — the social-preview tags belong to this increment, not before it

**Swept on the owner's question: the landing page has `charset`, `viewport`, `title`, `description`,
`icon` and a correct `<html lang="en">`, and no Open Graph or Twitter Card tags at all.** A link to it
shared in a chat client or on social renders as a bare URL — no card, no description, no image. **The
hero and the four story panels are invisible at exactly the moment the page is being passed around.**

**They land here rather than earlier because `og:url` and `og:image` must be absolute**, and both
change with the domain. **Adding them before the move means writing them twice.** This increment
already rewrites every absolute-URL surface; these join that list:

- `og:title`, `og:description`, `og:type`, **`og:url`** (absolute), **`og:image`** (absolute — the
  16:9 hero is the obvious candidate, and it is already a shipped asset)
- `twitter:card` (`summary_large_image`), and whichever of title/description/image that needs
- `<link rel="canonical">` — absolute
- `theme-color`, which the light/dark work now makes meaningful

**`og:image` must be a real absolute URL to a real asset**, checked by fetching it after the move —
a card that 404s is worse than no card.

### 7.2b RULED 2026-09-08 — no translation, and the criterion for revisiting

**The owner asked whether i18n should wait. It should, and the reason is not resource limits.**

**A translated landing page onto 465 untranslated documentation files is worse than a consistent
English site** — it invites a reader in, then strands them. And this is a tool where a stale
translation is dangerous rather than merely unhelpful: a mistranslated *"not yet a place to keep
history you cannot lose"* could cost someone their work.

**`lang="en"` is already correct**, so browsers offer machine translation. That is the honest fallback
— visibly machine-made, never mistaken for the project's own claim.

**RULED: the criterion for revisiting is not available resource but a named maintainer for a specific
locale, willing to own it continuously.** Translation without an owner decays into exactly the state
above.

**The landing page itself needs nothing.** Swept: its only absolute links are to `github.com`, which
the move does not touch, and its `docs/…` links are relative and travel with it.

**Timing, and it is now favourable:** `homepage` is frozen per published version and 0.36.0 already
shipped carrying `prikk-vcs.github.io`. **Landing the move before 0.37.0 means no released version
straddles the change.**

**The block is unchanged and is not ours:** `prikk.org` has no DNS record of any kind, re-checked
2026-09-08. **§7's ordering constraint stands — neither half may land early**, because
`site-url = "/docs/"` breaks the 404 page's `<base>` while the book is still served under `/prikk/`.

### 7.2d DELIVERED 2026-09-08 — increment 5 is done and verified live

**Delivered at `17d2a51`.** `homepage` → `https://prikk.org/`, `site-url` → `/docs/`, `README.md`'s
three sites, `release_notes.rs`'s compatibility link, and §7.2a's full meta-tag set.

**Verified on the live site after deploy, not from the diff:**

- **`/docs/404.html` now reads `<base href="/docs/">`** — it read `/prikk/docs/` before. This was the
  one control that could not be checked from a local commit, and the implementing round said so
  rather than reporting the value its commit would eventually produce.
- `prikk.org/` and `prikk.org/docs/` both 200; the old host still 301s to the apex.
- `canonical`, `og:url` and `og:image` are absolute and correct; **`og:image` returns 200.**

**A pre-existing defect found by the round, not by this RFC.** `release_notes.rs`'s link was
`…/prikk/reference/release-compatibility.html` — **missing `/docs/`**, so it 301s to a 404. **Broken
in the notes of every release since it was added**, and broken before the move too: on the old host
the book lived at `/prikk/docs/`. §7.2 flagged that file as the one that hides because it is Rust
rather than documentation; **it was hiding worse than a stale hostname.**

**Still open, neither owned by this RFC:**

1. **`www.prikk.org` has no TLS certificate.** DNS is correct; this is provisioning. Owner-side.
2. **No site-root `404.html`** (§7.2c finding 3) — a mistyped `prikk.org` URL renders GitHub's generic
   page. **Wants its own decision.**

### 7.2e RULED 2026-09-08 — the site gets a 404 page, and it lives at the landing root

**§7.2c finding 3 is now scheduled.** Any mistyped `prikk.org` URL renders GitHub's generic
*"Page not found · GitHub Pages"*. **This is the project's own front door failing**, and it is
reachable from every wrong link anyone ever writes.

**GitHub Pages serves only the site-root `404.html`.** `docs.yml` stages `docs/landing/` at the
artifact root, so **`docs/landing/404.html` is the file** — no workflow change is needed.

**This also explains why increment 5's `site-url` fix had no visible effect.** mdBook generates
`book/404.html`, which stages to `/docs/404.html` — **a path GitHub never serves.** Correcting its
`<base>` was right (the file should not carry a wrong value) but §7's stated symptom, an unstyled 404
under `/docs/`, does not occur and never did.

**RULED — the constraint that decides the design: a 404 is served at an arbitrary path.** A visitor
who mistypes `/docs/guide/instal.html` gets this page **at that URL**, so **every link and asset
reference in it must be root-relative or absolute.** A relative `href` resolves against wherever the
miss happened and breaks. **This is the same class of error as the `<base>` problem the RFC has
already been wrong about once.**

**RULED — it carries its own minimal CSS, and does not import the landing page's.** §7.2's CSS answer
(inline, 10.6 KiB gzipped) was reasoned for a single page, and a 404 is the second. **Extracting a
shared stylesheet would put a request on the landing page's critical path to save bytes on a page
almost nobody loads** — the wrong trade. **The 404 duplicates the colour tokens and a little layout,
nothing more.**

**Accepted cost, stated rather than discovered later:** the duplicated tokens can drift from
`index.html`'s. **That is cheaper than the alternative and it is a visual-only risk**, but a palette
change must remember this file.

**Not ruled:** the wording, and whether it offers anything beyond the two routes a lost visitor
wants — the landing page and the docs index.

## 8. The seam with RFC 135

The landing page's last instruction is an install command. **RFC 135 owns everything after it** — what
a new user meets before anything works, including that none of the 23 commands generates or derives a
key.

**The consequence for this RFC:** the landing page must not promise a first-run experience RFC 135 has
not built. Its install section ends at *installed*, not at *working*. If RFC 135 later produces a
`prikk setup`-shaped entrance, the landing page gains one line and no more.

## 10. AMENDED 2026-09-07 — the visual finishing pass, and why §9 no longer excludes it

**§9 put visual design out of scope as "settled in the drafts review".** The project owner commissioned
an external visual/UX review of the published page
(`.git-exclude/external-communication/external-arch/receive/audit-20260907-landing-page/`) and
instructed that its findings be acted on. **§9's exclusion is lifted for a finishing pass, and only for
that** — content, the truth mechanism, and the three-surface division are untouched.

**The review is accepted on its diagnosis.** Every checkable claim was re-derived by the architect
against `docs/landing/index.html` and reproduces exactly:

- `.panels li{width:170px}` with `gap:28px` over five sources of `270×170`, `150×310`, `240×300`,
  `265×400`, `340×400` renders at heights **~107px to ~351px**, captions at five different `y`.
- **The desktop overflow is arithmetic, not opinion**: `5×170 + 4×28 = 962px` against a content width of
  `1000 − 2×28 = 944px`, inside `.panels{overflow-x:auto}`.
- `@keyframes settle` ends `94%,100%{opacity:0}` on `infinite` — **arriving blocks vanish, forever.**
- **`--clay` is used exactly once in 436 lines**, at `.blk.clay{fill:var(--clay)}` inside the hero SVG.
  The accent appears nowhere in the page's own chrome.

### 10.1 RULED — finding A's fix, which is neither the current state nor the review's recommendation

**The diagnosis is right and the split was the architect's own recommendation.** §3's table records
*"Story image | used, split into panels with its text as real HTML"*; that came from the drafts review,
and the reviewer is correct that following it to the letter produced the bad result.

**But their recommended fix — use `prikk-attraction-story.png` whole as one `<img>` — is REFUSED, and
the reason is one their own scope prevented them from seeing.** They wrote *"I do not re-open its
content decisions"* and judged the content sound. **That image bakes in all three false claims §4.1
exists to have removed** — its bottom strip reads *"Isolated Workspaces — Safe & Parallel"*,
*"Effortless Review — Clear Changes"*, *"Confident Merge"* — plus a headline duplicating the page's own
`h1`, and text that is unselectable and inaccessible.

**Using it whole would reintroduce, as pixels, exactly what this RFC removed as prose.** That is also
the reason the split existed: it let the five story stages be used while the false-claim strip and the
duplicate headline were dropped. **The reason was right; the execution broke the story.**

**RULED — the artwork is re-rendered as one image containing artwork only:** the five stages on their
shared ground line with the connecting light paths intact, **and no baked text of any kind** — no
wordmark, no headline, no chips, no captions, no feature strip. Chips and captions are real HTML beneath
it, as they are now. **This keeps the flow, the truth mechanism, and accessibility at once**, which
neither the current page nor the review's recommendation does.

### 10.1a OWNER'S FINDING 2026-09-07 — the crops are unnatural, and layout alone cannot fix them

**The project owner: *"the current cropped images are ugly because unnatural to human. They should be
beautified following the original image."*** Confirmed at the assets, and it is the more fundamental
half of finding A.

**`panel-2-patches.webp` (150×310) is the clearest case.** The original's connector curves — which
flowed from the four patch cards into the "Integrated" cube — **are cut off mid-stroke at the right
edge and lead nowhere**. The patch cards themselves are clipped mid-shape. A pale rectangular backdrop
survives the crop and reads on the page as an accidental card, matching nothing around it. Every panel
is a **fragment of one scene**, not a composed picture: the original's shared ground line is cut
through, and its left-to-right light path is severed four times.

**This corrects the architect's own framing of the fallback.** §10.1 offered the reviewer's five-cell
grid as an adequate alternative. **It is not, on its own.** Aligning fragments to a shared baseline
fixes the *geometry* and leaves **severed connectors, clipped cards and mismatched backdrops exactly
where they are.** A tidy row of unnatural pictures is still unnatural.

**RULED — the standard, whichever option is taken: every image on the page must be complete in itself
and must follow the original artwork's composition, warmth and light.** No crop of a larger scene
qualifies, however well it is framed by CSS.

**So the fallback is amended:** if the single whole-artwork re-render is not available, the five panels
may stay **only if each is re-rendered as a properly composed picture in the original's style** — its
own ground, its own resolved framing, nothing running off an edge — and then laid out in the
reviewer's grid. **The grid is the layout fix; it was never the picture fix, and presenting it as a
standalone option was the architect's error.**

### 10.1b The story section is BLOCKED on an asset, and the rest is not

**Both options §10.1 permits are illustration work, and the original artwork is the owner's.** The dev
team writes markup; neither a whole re-render nor five re-rendered panels is theirs to produce.

**So the pass is split** (`visual-finishing-pass-handoff-v1.md`): the hero animation, the chrome warmth
pass, the terminal notes and the contrast measurement proceed now — all HTML/CSS/SVG, all independent
of how the story section resolves. **The story section is held, and the ragged strip stays until the
asset exists**, because every available interim is worse: the grid over the existing crops leaves them
unnatural (§10.1a), and narrowing the panels hides the overflow rather than fixing it.

**The asset is specified in the handoff's §2.3** — artwork only, one shared ground line, connections
intact, landscape near the original's `1536×1024`, `.webp`, tens of KB. **And no text of any kind.**
That last requirement is not stylistic: the original's baked feature strip is where three of §4.1's
false claims live, and **an image cannot be checked by the gate that keeps this page honest.** Text in
artwork is the route by which a removed claim returns without anything noticing.

### 10.1b-i CORRECTED 2026-09-07 — the architect overstated the requirement; re-cropping is enough

**The project owner: *"we have complete story image as a single file... Is what to do just to crop
naturally and then fill in the surrounding areas to create a clean, seamless look?"*** **Yes. That is
sufficient, and §10.1's framing was wrong to imply otherwise.**

§10.1 called for the artwork to be **"re-rendered"**, and §10.1a extended that to the panels. **That
overstated it.** The source is a single `1536×1024` image that already contains all five stages at one
scale on one ground line. **Producing five clean panels from it is image editing — crop and extend the
background — not illustration.** The architect framed a smaller task as a larger one and then declared
himself unable to do it.

**What "naturally" has to mean, though, is more than "don't cut things", and this is the part worth
being precise about.** Measured across the five committed crops:

| panel | source px | aspect ratio | height at a uniform 170px width |
|---|---|---:|---:|
| 1 start | 270×170 | **1.59** | 107px |
| 2 patches | 150×310 | **0.48** | 351px |
| 3 integrated | 240×300 | 0.80 | 212px |
| 4 growing | 265×400 | 0.66 | 257px |
| 5 shining | 340×400 | 0.85 | 200px |

**A 3.3× spread in aspect ratio is the single root cause** of the ragged heights, the floating captions
and the inconsistent apparent scale. So the re-crop must satisfy three things:

1. **One aspect ratio for all five.** Pick one frame and crop every stage to it. This alone fixes the
   raggedness, the floating captions, and the 962-vs-944 overflow.
2. **One subject scale.** The stages sit at consistent size in the source; the tight crops destroyed
   that, so at equal display width panel 1's plinth looks small and panel 5's cube fills its frame.
   **They must read as one sequence, not five zoom levels.**
3. **Nothing severed — and the connectors are the reason.** The arrows and light paths live *between*
   stages in the source. **Crop to exclude them entirely** rather than clipping them mid-curve, which
   is what left `panel-2-patches.webp` with curves running off its right edge into nothing. Extend the
   background where a crop needs breathing room.

**With those three, the reviewer's five-cell grid becomes the right layout** rather than the tidying of
unnatural fragments §10.1a refused: equal cells, shared baseline, ordered single-column collapse.

**§10.1d's option C (a hand-authored SVG) is therefore no longer the only thing available without new
illustration**, and is demoted to what it always was — an alternative aesthetic direction, not an
escape from a block that turned out not to exist.

### 10.1b-ii DELIVERED 2026-09-07 (`6055149`), with one required follow-up

**The five panels were re-cropped from the existing source, as §10.1b-i ruled** — one `420×420` canvas
each, native-pixel scale, padded not stretched, bottom-anchored. **Verified by bounding-box measurement
rather than from the render**: nothing is clipped, and `panel-2-patches.webp` — whose connector curves
ran off the edge into nothing — now carries **140px of clean right margin**. Assets fell from 44,554 B
to **32,564 B (−27%)**.

**The implementing round contradicted this RFC's own instruction, correctly.** §10.1a carried the
external reviewer's `align-items:end` verbatim. **It misaligns the artwork here**: `align-items`
positions each grid item's whole box, and captions wrapping to one line for two panels and two for
three make the boxes differ by 20.25px — so end-aligning pushes the shorter-captioned panels' *images*
down by a caption line. The advice is right where the image is the last element in the item; it is
wrong for this markup, and **neither the reviewer nor the architect checked which we had.**

**REQUIRED follow-up** (`story-panel-height-handoff-v2.md`): `.panels img` sets no height, so the
markup's `height="420"` presentational hint applies and **`aspect-ratio:1/1` is ignored — it only fills
a dimension that is `auto`.** Each panel therefore reserves a **~425px box to show 148px of artwork**,
about 300px of dead space per panel. **The evidence was in the implementing round's own report** — an
`imgTop`→`capTop` gap of 434px where a square image in a 172.8px column gives ~187px. One line:
`height:auto`.

### 10.1b-iii §2 COMPLETE 2026-09-07 (`bae123f`) — and the pass with it

`height:auto` added; the box is square, and the ~300px of dead space per panel is gone. **Measured
independently from the renders**, not read from the report: the chip→artwork gap fell from **168px to
36px** and the artwork→caption gap from **135px to 19px** (the caption's own margin), while the artwork
band itself is unchanged at ~150px — **nothing was shrunk to achieve it.** The section is ~250px
smaller than when the pass began.

**The comment shipped around the one-line fix is worth more than the fix**, because it records *how it
was found* — the previous round's render measurements not matching the 1:1 model. A future reader who
deletes `height:auto` as redundant beside `aspect-ratio` will find the reason it is not.

**The §2 thread took four corrections, each catching what the previous party could not see:**

| | |
|---|---|
| The external review's diagnosis | accepted; every checkable claim re-derived |
| Its recommended fix (use the image whole) | **refused** — that artwork bakes in three claims §4.1 removed, which their own content scope hid from them |
| The architect's "must be re-rendered" framing | **corrected by the owner** — re-cropping the existing source was enough, and the architect had invented a block |
| The architect's `align-items:end` instruction | **corrected by the dev team** — it misaligns the artwork in this markup order |
| The architect's review of the result | **found the 420px box** that the round's own reported numbers had already shown |

### 10.1c A third defect, and a gap in the architect's own asset spec

**Found 2026-09-07 while answering "who can make the artwork": the story panels do not theme-adapt.**
They are baked light-background rasters, and the page has a dark theme. In the external architect's own
dark render they are **five bright cream rectangles glowing out of a dark page** — visibly worse than
the ragged strip is in light mode. The header logo has the same problem.

**Neither the external review nor the architect named this.** The review's dark-mode note was about
`--beige` as a card fill; the panels' own non-adaptation went unremarked in both light-mode analyses.

**It is also a gap in §10.1b's asset spec, which is the architect's.** That spec says nothing about
background or theme, so art following it exactly would still glow white on the dark page.

**And under §10.1b-i's re-crop route this needs the simplest answer, not the most thorough one.** The
source artwork's cream ground is part of the picture; making it transparent would mean removing the
glow that is the point of the "shining" stage. **Ruled: give the story section a fixed paper background
in both themes** — one CSS rule — so the artwork sits on paper deliberately rather than punching five
bright holes in a dark page. Two asset variants are not required, and a transparent background is not
required.

### 10.1d The three ways to unblock §2, and the third is available now

**The architect cannot produce artwork in the original's rendered-3D style.** That is illustration work.
Three options, and the choice is the owner's because the identity and the original art are theirs.

| | Who | Style | Weight | Theme-adapts |
|---|---|---|---|---|
| **A. One re-rendered artwork, whole** | owner or an illustrator | matches draft-02's warmth | tens of KB (target) | only if transparent or two variants (§10.1c) |
| **B. Five re-rendered panels** | owner or an illustrator | same | ≥ today's 60 KB | same caveat |
| **C. Hand-authored SVG, in the hero's own language** | **the architect, now** | flat rounded tiles + connecting paths, sage/beige/clay | **~3-5 KB** | **yes, by construction** — CSS variables |

**Option C is not a fallback invented to escape the block; the page already proves it works.** The
rebuilt hero mark is exactly this — **1,362 bytes** of rounded `rect`s, `path`s and a clay node, filled
from CSS variables, which is why it adapts to both themes while the panels beside it do not. A
five-stage story in the same language would give the page **one visual language throughout**, which is
the coherence the external review said it lacked — reached from the other direction.

**The trade is real and it is the owner's to weigh.** The external review offered both directions:
*"either warm the chrome up to meet the art, or calm the art down to meet the chrome."* §10.2 chose to
warm the chrome, and that half has shipped. **Option C is the other half of the same sentence** — and
it would lose draft-02's rendered warmth, which the owner liked and which A and B keep.

**Nothing is decided here.** Option C is offered because it is buildable today and costs nothing to
discard; A and B remain preferable if the owner wants the rendered look and can supply the asset.

### 10.2 RULED — the rest

- **Finding B — the hero animation is a defect, not a preference.** Blocks that arrive and then fade to
  `opacity:0` contradict the page's own message and the direction's §17. **Arriving blocks must stay.**
  A single settle on load, then static, satisfies "still look beautiful when paused".
- **Finding D's terminal clipping is a defect**: the two `note:` lines are the honest caveats, and they
  are the part being hidden. Shorten or wrap.
- **Finding D's dark-mode contrast is objective**: `--ink-soft` on `--paper` must be **measured** against
  WCAG AA in both themes, not judged.
- **Finding C — warmth and texture — is accepted in its mechanical parts** (fewer hard rules, soft
  tonal shifts, deliberate sparing use of `--clay`). **The aesthetic direction itself is the owner's**,
  who owns the identity and approved both drafts. The reviewer's framing — *warm the chrome up to meet
  the art* rather than calm the art down — is endorsed by the architect but is a redirectable choice,
  and the owner's stated value is "clean over rich".
- **Finding D's heading cadence is taste, not a defect.** Optional.

### 10.3 What must not be lost

The reviewer's §E, verbatim as a constraint on the pass: content truth, the maturity note up front, real
terminal output, the copy button with its live region, reduced-motion as the opt-in default, the
reflow-friendly command wrap, semantic sections and heading order. **A styling pass that breaks one of
these has failed regardless of how it looks.**

## 11. AMENDED 2026-09-08 — the owner's second visual pass: a hero photograph, a four-panel story, and a reordered page

**The owner supplied new artwork and a proposed section order** (`.git-exclude/tasks/architect/landing-20260908/`).
Everything in §11 is ruled from the assets and the live page as they stand today, not from the proposal
text alone.

### 11a. The section order is adopted as proposed, because the names were already ours

**The owner's section names are the page's existing `eyebrow` strings**, which makes the mapping exact
and leaves nothing to interpret:

| owner's name | section | position now → proposed |
|---|---|---:|
| Hero — *"Version control, patch by patch"* | `.hero` | 1 → 1 |
| *"Get started"* | `.install` (`id="start"`) | **7 → 2** |
| *"A calmer way to work"* | `.flow` | 3 → 3 |
| *"The idea"* | `.principles` | **2 → 4** |
| *"Patch by patch"* | `.story` | 5 → 5 |
| *"What you get"* | `.features` | 6 → 6 |
| *"Try it"* | `.terminal-section` | **4 → 7** |
| Closing | `.closing` | 8 → 8 |

**RULED: move the sections whole. Invent no new section and split none.** The owner offered *"or at
least a tiny digest with a few command lines"* as a fallback; taking it would mean authoring a second
"Get started" and leaving the original with a duplicate eyebrow. **Moving `.install` intact is the
smaller change and it is reversible.**

**One risk, named rather than pre-empted:** the hero closes on *"Early implementation. Not yet a place
to keep history you cannot lose."* Position 2 then asks the reader to install it. **If that reads
badly in the staged page, trimming `.install` to a digest is the follow-up** — but it should be seen
in place first, not designed around in advance.

### 11b. RULED — the fourth panel is **"Grow"**, and the heading changes with it

The new set has **four** panels; the page has five. The owner asked whether the fourth is *"Grow and
Shine"* or *"Shine"*.

**Neither: it is "Grow."** *Shine* names a lighting treatment that panel 3 already carries — the
glowing connections are what "integrated and validated" looks like — so it does not distinguish the
last panel. What is new in panel 4 is the leaves — the cube is alive and spreading. **"Grow" names
that, and completes the arc: Start → Patches → Integrate → Grow.**

**The four chip labels are exactly `Start`, `Patches`, `Integrate`, `Grow`.** Nothing else in this
section is a label; the surrounding sentences are the reasoning for the choice.

**Consequence the proposal does not mention:** `<h2>Projects grow. Then they shine.</h2>` is a
*two-beat* heading built for a two-panel ending that no longer exists. **It must change with the
panels.** The heading is brand voice and the owner's to settle; the RFC records only that leaving it
is not an option.

### 11c. FINDING — the two "light" heroes are different times of day

**This is a defect in the asset set and only the owner can fix it.**

- `prikk-hero-16_9-light-01.png` — **bright daylight**: blue sky, white cloud, sun high and right.
- `prikk-hero-9_16-light-01.png` — **dusk**: violet-and-orange sky, sun on the horizon, lit water.

The 9:16 "light" is the same scene as the 9:16 dark at a brighter exposure — **a good pair.** The 16:9
light is the odd one out of all four. **So "light theme" means daylight on a desktop and sunset on a
phone**, and one reader rotating a tablet crosses that boundary.

**The composition change between form factors is right and should be kept** — cards flow left-to-right
into the cube at 16:9, and descend onto it at 9:16. That is a genuine re-composition rather than a
crop, which is what the earlier cropping round failed to do.

### 11d. CORRECTED 2026-09-08 — there is no single background to match, so stop matching

**My first ruling here was wrong, and wrong twice.** It said the dark panels' ground is `#1d2a28` and
that `.story`'s dark background should be set to it. **The value came from one sampled point, and the
premise that one value exists does not hold.** The owner questioned it; measurement settled it.

**Mean ground of a 60x60 patch, top and bottom of every cell:**

| | panel 1 | panel 2 | panel 3 | panel 4 |
|---|---|---|---|---|
| light, top | 253,246,238 | 253,245,236 | 253,245,237 | 253,246,237 |
| light, bottom | 249,239,229 | 246,235,225 | **245,232,220** | 250,237,224 |
| dark, top | 25,37,38 | 26,37,36 | 26,37,36 | **38,47,42** |
| dark, bottom | 23,35,35 | **34,40,36** | 25,35,35 | 32,42,39 |

**Two things the single sample hid.** Every panel has an internal **top-to-bottom gradient** — a warm
floor shading under the subject — so no panel has *one* ground of its own either. And the panels differ
from each other: **dark panel 4's top is 13/255 lighter than its neighbours**, because its halo lifts
the entire frame, and dark panel 2's *bottom* is the same kind of outlier.

**In the light row the tops agree within 2/255 and the difference is invisible. In the dark row it is
not** — small absolute deltas are perceptually large against a dark ground.

**RULED: `.story` stops trying to match the artwork, and the panels become panels.** Give each image a
defined bound — rounded corners and the grid gap already present — and let the section background be
the ordinary theme token. **A warm cream card on the light page and a darker inset on the dark page
both read as deliberate; a near-match that is wrong by 13/255 on one panel out of eight reads as a
mistake.**

**This retires the whole defect class rather than re-solving it.** §10.1a (unnatural crops), §10.1d
(the height that never applied) and this are three rounds of the same underlying attempt — making
raster artwork behave as though it were page background. **It cannot be made robust: any future
regeneration of the artwork moves the target again.** A bounded panel is immune to that, and it is the
cheaper thing to keep true.

**§10.1c's pin comes off regardless** — it existed because the old panels could not theme-adapt, and
these can.

### 11e. The sheet's geometry, measured

`prikk-story-image-set-01-light-dark-compat.png` is 1536×1024: **four columns × two rows, cells
exactly 384×512.** Light row `y=0`, dark row `y=512` — a hard transition, no seam. Columns begin at
`x = 0, 384, 768, 1152`, with a **≈2px near-white seam on each column boundary**, so crops must inset
horizontally rather than cut on the nominal edge.

**Panels change from 1:1 to 3:4.** `.panels img{aspect-ratio:1/1}` and
`.panels{grid-template-columns:repeat(5,1fr)}` are both wrong for this set. **§10's own round found
`aspect-ratio` silently not applying**; that is the thing to re-verify by measurement here, not by
reading the rule.

### 11f. The `.hero-art` SVG — the right-side defect, located

The owner ruled it re-drawn and moved rather than removed, and named the right-side connection as
unnatural. **It is, and the reason is specific.**

The left connector is `M46 108 V82 H114 V66` — it **rises clear of the blocks to `y=82`**, travels in
open space, then enters the upper block, with a node at the bend `(114,82)`.

The right connector is `M182 108 H250` — **`y=108` is the top edge of both blocks**, so it runs *along
their tops* for 26px on each side rather than through open space, and it carries **no node at its
bend**. Two connectors, two different idioms.

**The fix is to mirror the left**: rise to `y=82`, travel, descend — `M182 108 V82 H250 V66` — with a
node at `(250,82)`. It does not collide, because the left path occupies `y=82` only between `x=46` and
`x=114`.

### 11g. RULED — every asset ships as WebP, and this is not negotiable

The four hero PNGs are **1.6–2.0 MB each, ≈7 MB for the set**. The panels already on the page are WebP
at 2–12 KB. **A landing page whose entrance image is heavier than the entire rest of the site fails the
purpose §1 gives it.** Convert; keep the PNGs as sources under `.git-exclude/`, which does not ship.

## 12. RULED 2026-09-08 — what the landing page may claim, verified against the code

**The owner asked whether prikk can offer safe merge *and* ownership of the history record.** Both, with
one boundary. **This section exists so a later copy pass cannot write a stronger version than the code
supports** — every line below was checked at source, not recalled.

### 12a. Safe merge — yes, and the honest verb is "named", not "clean"

`ConflictWitnessKind` (`crates/prikk-store/src/patch_algebra/types.rs`) generates **twelve typed
witnesses** from one macro so a thirteenth cannot be added without a label: `same-path-create`,
`node-id-reuse`, `live-state-mismatch`, `kind-mismatch`, `mode-mismatch`, `blob-mismatch`,
`text-span-overlap`, `text-anchor-stale`, `delete-mutation-conflict`, `unsupported-operation`,
`malformed-operation`, `unknown-relation`. **`prikk merge-evidence` is read-only and reports them
before anything merges.**

**Claimable:** a conflict is *named up front* rather than discovered later inside a plausible-but-wrong
result, and a content-anchored edit survives a rename because spans are not line ranges.
**Not claimable: that merges are clean or always succeed.** Conflicts still happen; prikk's property is
that they are typed and visible, not absent.

### 12b. Ownership of the record — yes, and it means *unmediated*, not *durable*

**Claimable, each backed:**

- **No forge owns the history.** Repositories are anonymous and sync is artifact-based — bytes you
  hold, not an account on someone's server.
- **Anyone can verify offline.** The host's word is never required.
- **Undo is recorded, not erased.** A rollback becomes sealed history rather than a force-push that
  removes the evidence.
- **Authorship is attributable.** Ed25519 over the patch's own content id
  (`author/author_signing.rs`), and D8 binds one `key_id` to exactly one public key — a signature
  failing against *recorded* material is a hard verification failure, not a warning.

### 12c. The four things that must NOT be claimed

1. **No trusted timestamp.** `Signature::created_at` is fixed at `0`, documented *"Advisory only
   (never used as authoritative audit time)"*. **Prikk proves who, never when.**
2. **An unsigned patch is not a failure.** `verify_author_signature` returns `Ok(None)` — the check
   does not apply rather than failing.
3. **An unrecorded `key_id` yields `sound: false`, explicitly not a failure.** The claim is recorded,
   not proven; attribution is only as strong as the key material actually recorded.
4. **Nothing about durability.** The hero's own maturity note says this is *"not yet a place to keep
   history you cannot lose."* **Ownership is about who mediates the record, not whether it survives.**

### 12d. RULED — the `.story` heading

**`Bring changes together. Keep the story yours.`** — the owner's wording, replacing both
*"Projects grow. Then they shine."* (which promised a fifth panel §11b retired) and the architect's own
*"No surprise merges. The history stays yours."*

**The owner's correction was a viewpoint one and it was right:** the architect's line named a
*developer's* annoyance — merges that surprise me — where this section addresses a **project owner**.
An owner's two acts are bringing contributions together and holding the account of how the project got
here. **Both clauses are imperative, matching `Commit. Seal. Verify.`**, where the architect's was a
noun phrase plus a statement.

**It also removes an echo the architect missed**: *history* already appears in
*"Small changes. Clear history."* two sections earlier. *Story* does not appear in visible copy
anywhere else on the page, checked.

**Bound to both properties, and to neither overclaim.** *"Bring changes together"* is §12a's act
without claiming merges are clean or automatic; *"Keep the story yours"* is §12b's *unmediated*.
**Recorded caution:** *keep* could be misread as durability, which §12c.4 forbids — judged acceptable
because the object of *keep* is the ownership, not the story's survival, and the parallel imperative
reads as a pair of actions rather than a guarantee. **`The story stays yours` is the zero-risk
alternative if that reading ever proves wrong**, at the cost of the parallel.

## 13. AMENDED 2026-09-08 — the third visual pass: the artwork is barely visible, and the owner's reveal fixes it

**Three findings, all framing rather than assets.** The images are right; the page is hiding them.

### 13a. Measured — how little of the hero actually renders

At a 1400px viewport, measured from the round's own render rather than estimated:

```
hero photo box   1400 × 542  = 2.58:1
image            1672 × 941  = 1.78:1
cover scales the image to 788px tall against a 542px box
  -> 31.2% of the image height is discarded before any scrim applies
scrim  linear-gradient(100deg, cream 0%, cream 40%, transparent 75%)
  -> opaque to 560px, fading to 1050px, only the right 350px of 1400 unobstructed
```

**Two compounding losses.** The 16:9 composition runs cards left-to-right into the cube; the cards are
under the opaque band and a third of the frame is never drawn. **The owner's report that the artwork is
"almost hidden" is correct on desktop as well as mobile**, and the architect's earlier assessment of
the desktop hero as "strong" was wrong.

### 13b. RULED — the owner's content fade-in is adopted, and it is the accessible direction

**The owner proposed fading in the *content* over an already-visible photograph** — the image shows
first, the copy arrives after. The architect first evaluated the opposite (fading the scrim *out*) and
argued against it. **That objection does not apply to what was actually proposed, and the owner's
direction is the safe one by construction:**

- With content fading **in**, the `prefers-reduced-motion` fallback is *copy visible immediately* —
  today's legible state. **Fading the scrim out would have made the fallback the unreadable state.**
  Yours degrades to safe; the version argued against degraded to broken.
- **It needs no JavaScript.** A CSS animation with a delay is not a content decision, so the page's
  standing no-JS-for-content convention is untouched.

**RULED: adopt it, for both form factors.** It answers the desktop occlusion and the narrow-width case
— where the copy card covers roughly four-fifths of the 9:16 image — with one mechanism.

**Consequence: the hero photo stays a full-bleed background.** The architect's earlier proposal to give
it its own row is withdrawn; the reveal makes it unnecessary.

### 13c. RULED — the reveal does not fix the crop, and the crop must be fixed separately

**§13a's 31.2% loss is independent of the scrim.** However long the photo is uncovered, the top and
bottom of the composition are never rendered. **The hero must be sized so `cover` has little or
nothing left to cut.**

**RULED: the image wins, and the fold is not the constraint it looks like.** Matching the image's
aspect makes a 788px-tall hero at 1400px, which pushes the hero's own `Get started` below the fold on
most laptops. **That is acceptable, because the first screen already carries the action:** `.nav-cta`
(`index.html:302`) is a `Get started` link in the top nav (`href="#start"`), above the hero and
therefore above the fold on load at every viewport. **The nav is not sticky — checked, not assumed —
so this covers the first screen, not the whole scroll.** That is enough: a visitor who scrolls past it
is scrolling through the argument the CTA is the end of. **Cropping a third of the artwork to protect a duplicate CTA
would defeat the pass.**

**The bound:** size the hero from the image's aspect, capped viewport-relatively so it can never grow
absurd on a wide monitor. **Mechanism is the implementing round's; the property is that the image is
substantially whole and the residual crop is reported at named viewports.**

**This was briefly deferred to the owner and should not have been.** Hero geometry is a design call,
not a question of goals, priorities, scope, risk acceptance or release approval. **Recorded because the
error was mis-assigning authority, which is worth catching in itself.**

### 13d. Measured — the story panels sit low, and light is worse

Subject centre versus frame centre (256), per 378×512 cell, stable across detection thresholds 25-60:

| | Start | Patches | Integrate | Grow |
|---|---:|---:|---:|---:|
| **light** | **+112** | **+66** | **+62** | **+51** |
| dark | +82 | +40 | +42 | +14 |

**Both rows are bottom-heavy; light sits 20-37px lower still.** The owner saw it only in light because
pale empty space reads as a gap where dark space reads as depth — **the same placement, differently
legible.**

**RULED: fix it by framing, not by new artwork or re-cropped files.** The panels are 378×512 in a 3/4
box, so `cover` crops ≈1.6% and there is no slack to steer. A squarer box restores the slack and
`object-position` aims it, **using the delivered images untouched** — reversible, and no asset churn.

**Two constraints:** the box shape must be identical in both themes or the grid stops aligning, and
**dark needs its own `object-position`**, since a value centring light overshoots dark on `Grow`.

**One honest limit, recorded so it is not chased:** **`Start` cannot be centred at full frame height.**
Its subject sits so low that centring inside a 512px frame would need the frame cut to ≈286px.
Framing improves it from +112 to roughly +45 — better than dark is today — and **the remainder needs
new artwork for that panel, which is the owner's call and is not this round's work.**

## 13e. DELIVERED 2026-09-08, with two corrections to §13c's own expectations

**Delivered at `8b9ca94`, `d5d8e73`, `9c6faa2`.** The reveal is safe-by-default as §13b required —
`.hero-reveal{opacity:1}` is the base rule and only `prefers-reduced-motion: no-preference` animates
it. Panel framing verified from the shipped pixels by re-running §13d's own detection: Start **+48**
light / **+16** dark, every other panel within ±3 of centre in both themes.

**Correction 1 — the accepted cost did not occur.** §13c accepted the hero's `Get started` falling
below the fold. Measured, it stays above at both 800px and 900px viewport heights (top 519, bottom
563). **Recorded so nobody later fixes a problem that is not there.**

**Correction 2 — one cap cannot serve both ends, and that premise was mine.** Residual crop measured
**4.04% at 1400px, 0.00% at 1024px, 19.18% at narrow**. The implementing round declined to tune
`--hero-cap` to zero one row at the other's expense, and said so rather than choosing silently.
**They were right, and §13c's "single tunable value" is what forced the conflict** — it meant
*tunable*, not *forbidden a breakpoint override*.

**RULED: the narrow breakpoint takes its own cap.** The hero's aspect ratio is already
breakpoint-specific (`.hero{aspect-ratio:941/1672}`), so a narrow-specific ceiling is consistent with
the structure already present. At 390px the 9:16 ratio implies a 693px hero, which most phone
viewports exceed — **raising or dropping the cap there takes narrow crop to approximately zero without
touching the wide-monitor bound.**

**A defect the round found unprompted, worth keeping in the record:** once `max-height` clamps an
aspect-ratio-derived height, a block box with no explicit `width` recomputes *width* to preserve the
ratio rather than staying full-bleed. At 2560px this left 937px of bare background beside the photo.
**Fixed with `width:100%`.** The test was not asked for by name — §13c's phrase *"cannot grow absurd
on a wide monitor"* was read as an instruction to try one.

## 13f. CORRECTED 2026-09-08 — squaring the box bought centring by spending the artwork's margins

**The owner reports the story images are too tight on all four sides, and that dark is worse.**
Measured on the shipped renders, subject margin inside the 201×200 image box:

| | top | bottom | left | right |
|---|---:|---:|---:|---:|
| light Start | 76 | 24 | 13 | 15 |
| light Patches | 34 | 30 | 14 | **6** |
| light Integrate | 15 | 12 | 17 | 12 |
| light Grow | **9** | **8** | 13 | 17 |
| dark Start | 45 | 27 | 16 | 15 |
| dark Patches | 19 | 19 | 14 | **8** |
| dark Integrate | **1** | **0** | 18 | 12 |
| dark Grow | **0** | **0** | 8 | 16 |

**Both reports confirmed. Dark Integrate and Grow touch the frame outright**, and dark tops (45/19/1/0)
run consistently under light's (76/34/15/9).

**The cause is §13d's own ruling and it is mine.** Squaring the box to create `object-position` slack
spends **134px of the 512px source height on crop** — and that crop is exactly the margin the artwork
carries around its subject. **In the source, bottom margins are only 26-63px**, so a 134px crop
removes them entirely. **Centring and margin are drawn from one budget; §13d bought the first with the
second without saying so.**

**The margin is recoverable — it is in the files.** Source subject extents per 378×512 cell, top and
bottom margin:

| | light | dark |
|---|---|---|
| Start | 273 / 47 | 219 / 53 |
| Patches | 195 / 62 | 144 / 63 |
| Integrate | 156 / 31 | 111 / 26 |
| Grow | 137 / 34 | **57 / 27** |

**RULED: margin wins over exact centring.** A subject flush against a frame edge reads as a mistake;
a subject a few pixels off centre does not. **Reduce the crop until every panel has visible margin on
all four sides in both themes, and accept whatever centring the remaining slack allows.**

**Not ruled: the box ratio.** It is a continuous trade — every pixel of crop returned is a pixel of
centring slack lost — and it must be chosen against measurement, not argued in advance. **§13d's
per-theme `object-position` structure stays; only its budget changes.**

**Recorded limit:** `Grow` dark carries just **57/27** of margin in the source, so even at zero crop
its rendered margin is small. **If it cannot reach the target, that is artwork, not CSS** — the same
category as `Start`'s residual offset, and the owner's call.

## 13g. CORRECTED 2026-09-08 — §13f was a set-wide ruling for a two-panel problem, and it is reverted

**§13f ruled "margin wins over exact centring" and told the implementing round to reduce the crop
set-wide. That was wrong, and the error is the shape of the ruling, not the round that carried it
out.**

**Measured, and it is the number §13f should have established first — subject height against the
square box's 378px window:**

| | Start | Patches | Integrate | Grow |
|---|---:|---:|---:|---:|
| light | 191 | 254 | 324 | 340 |
| dark | 239 | 304 | 374 | **427** |

**Only two of eight subjects exceed the window: dark `Integrate` (374) and dark `Grow` (427).** The
other six fit with room. **§13f treated a two-panel problem as a set-wide one and spent six good
panels to fix two.**

**What the set-wide change actually produced**, measured on the shipped render at `4/5`
(box 201×251):

| | top | bottom | off-centre |
|---|---:|---:|---:|
| light Start | 126 | **0** | +63.0 |
| light Patches | 83 | **0** | +41.5 |
| light Integrate | 62 | **0** | +31.0 |
| light Grow | 51 | **0** | +25.5 |

**Every light panel runs to the bottom edge with a large gap above** — which is the owner's *original*
complaint from before §13d, restored. The round reported the centring loss honestly in its own table;
the ruling is what made it inevitable.

**RULED: revert to the square box.** Round 4's framing stands — light margins 76/34/15/9 top and
24/30/12/8 bottom, dark 45/27 and 19/19 for `Start` and `Patches`, and every panel within ±3 of centre
except `Start`. **Reverted directly rather than handed back**, being a correction to the architect's
own ruling restoring a state already reviewed and accepted.

**RULED: dark `Integrate` and dark `Grow` are artwork, not CSS.** Their subjects are physically larger
than any window that keeps the other six centred — 427px of a 512px cell for `Grow`. **No framing
choice reaches them.** They join `Start`'s residual offset as the second owner-facing artwork item:
**the subjects want to be smaller within their own cells.**

**The lesson, recorded because it is general:** *"fix the tight ones"* and *"change the box"* are not
the same instruction. **§13f should have asked which panels failed and why before ruling how the set
should change** — the answer was two, and it was in the source geometry the RFC already had.

## 13h. RULED 2026-09-08 — the replacement artwork, and the box stays square

**The owner supplied eight replacement panels** (`prikk-story-image-set-{light,dark}-0{1..4}.png`),
one per panel per theme, closing §13g's artwork item. **Measured rather than accepted:** subject
margin, normalised to a 256-unit frame.

| | top | bottom | left | right | off-centre |
|---|---:|---:|---:|---:|---:|
| light Start | 124 | 60 | 57 | 59 | +32.0 |
| light Patches | 93 | 65 | 58 | 53 | +14.0 |
| light Integrate | 78 | 53 | 62 | 57 | +12.5 |
| light Grow | 71 | 55 | 58 | 59 | +8.0 |
| dark Start | 115 | 54 | 63 | 59 | +30.5 |
| dark Patches | 74 | 60 | 54 | 45 | +7.0 |
| dark Integrate | 71 | 35 | 58 | 49 | +18.0 |
| dark Grow | 57 | 42 | 51 | 57 | +7.5 |

**Bottom margins are 35-65 of 256 (14-25%) against the old set's 26-63 of 512 (5-12%)** — roughly
tripled proportionally. **The owner's "rich margin" is confirmed.**

### 13h.1 The trade, computed this time rather than ruled blind

**§13f's mistake was ruling a box ratio without computing what it cost.** Done properly, across
candidate ratios, taking the worst panel of the eight on each axis:

| box ratio | worst margin | worst off-centre (of 256) |
|---|---:|---:|
| 0.90 | 15.0% | 31.3 |
| 0.95 | 14.4% | 24.6 |
| **1.00 (square)** | **12.6%** | **18.6** |
| 1.05 | 10.7% | 13.1 |
| 1.10 | 8.8% | 8.2 |
| 1.20 | 5.1% | 0.0 |

**RULED: the box stays square.** At 1.00 **six of the eight panels centre exactly** and the worst
margin across all eight is 12.6% — against the old artwork's 0/0px on dark `Grow` at the same ratio.
**Going wider buys the last two panels' centring by spending everyone's margin**, which is the trade
§13f got wrong; going narrower buys margin nobody needs at a centring cost that shows.

**The CSS box does not change.** `aspect-ratio: 1/1` is what ships today after §13g's revert.
**This round replaces assets and re-derives `object-position`, nothing else.**

### 13h.2 `Start` stays slightly low, and that is now acceptable

**`Start` clamps at +18.6 (light) and +7.3 (dark) of a 256 frame** — its subject sits low in the source
and is small, so no square-box framing fully centres it. **It is also the panel with the largest
margins of the eight (26%).** A small subject, generously surrounded, sitting a little below centre
does not read as a mistake the way a subject flush against the frame did.

**Both artwork items from §13g are closed by this.** `Start`'s residual is no longer an owner-facing
question, and dark `Integrate`/`Grow` — which physically exceeded the frame at 374px and 427px of a
512px cell — now sit at 14.2% and 12.6% margin.

## 13i. DELIVERED 2026-09-08 — the landing page is complete

**Delivered at `07a0aec`.** Eight replacement panels, eight re-derived `object-position` values, CSS
box unchanged. **§13h.1's arithmetic held against a real render:** six of eight panels within ±3 of
centre, `Start` the only exception in either theme, worst margin 11.4-13.4% against the predicted
12.6%. **81,852 bytes total, smaller than the old set's 87,526 despite the richer margins.**

**Both artwork items §13g raised are closed.**

**The control skipped in two prior rounds was run by the implementing round itself**, with the
detector calibrated against §13h's own published numbers before being trusted — and it caught a 7px
scrollbar discrepancy that computed geometry would have missed. **Independently reproduced.**

**Recorded for whoever measures this artwork next: the detector is glow-sensitive.** Two independent
renders disagreed by ~13 units on light `Integrate` at threshold 40 while agreeing it is centred at
threshold 50. **Sweep the threshold; do not trust one reading on artwork with bloom.**

**What remains is not landing-page work:** increment 5 (blocked on `prikk.org` DNS, scope in
§7.2/§7.2a/§7.2b) and the `.hero-art` drawing's appearance.

## 13j. CORRECTED 2026-09-08 — I optimised a bounding box; the eye reads mass and scale

**The owner reports the shipped panels are too small, and the content sits too high — a little on
light, seriously on dark.** §13h.1 said six of eight were centred with 12.6% worst margin, and that
was measured correctly. **It measured the wrong things.**

### 13j.1 The axis never measured: subject scale

**Subject height as a fraction of the 201px frame, from the shipped render:**

| | Start | Patches | Integrate | Grow |
|---|---:|---:|---:|---:|
| light | **30.8%** | 42.3% | 53.7% | 57.2% |
| dark | 41.3% | 57.7% | 70.6% | **75.6%** |

**A 30.8%-to-75.6% spread across one set, and `Start` at 30.8% sits beside `Grow` at 57.2% in the same
row.** §13h.1 optimised *margin* and *centring* and never asked how much of the frame the subject
actually occupies — **the quantity that carries visual impact.** More margin and a bigger subject are
in direct opposition, and I tuned only one of them.

### 13j.2 Why "centred" reads as "too high" on dark

**A bounding box is not perceived mass.** Comparing the bbox centre against a centroid weighted by
each row's deviation from the background:

| | bbox centre | mass centroid | centroid sits |
|---|---:|---:|---|
| dark Start | +6.5px | −7.1px | **13.6px above** |
| dark Integrate | −1.0px | −9.7px | 8.7px above |
| dark Grow | −1.0px | −12.2px | 11.2px above |

**The detector counted the glow and ground-reflection below the subject; the eye does not.** The box is
centred while the visual weight sits up to **13.6px high on a 201px frame — 6.8%**, which is plainly
visible. **The dark artwork has more spill below, which is why dark is the worse half.**

### 13j.3 What I cannot explain, stated rather than papered over

**On light, the same measurement puts the mass centroid *below* the bbox centre by 6.5-9.3px** — which
should read low, not high. **The owner's report of "a little too high" on light does not reproduce in
any measurement I have.** I am not going to assert a mechanism I have not found.

**Consequence: vertical position stops being settled by my detector.** It is tuned against the render
and confirmed by the owner's eye. **A detector that has now disagreed with the owner twice does not get
the deciding vote on a question of appearance.**

### 13j.4 RULED

1. **Subject scale is a first-class target.** The set must be visually consistent and the subject must
   dominate its frame. **Both are yours to hit against the render, not mine to specify as a number** —
   §13h.1's failure was exactly a number that looked right and was not.
2. **Zoom in CSS, not by re-cropping the assets.** The sources are good; the framing is wrong.
   Keeping the files intact keeps this reversible and keeps re-tuning cheap.
3. **Vertical position is tuned by mass, not by bounding box**, and the owner confirms it. §13j.2's
   centroid measurement is the better instrument; it is not the authority.
4. **Margin stays sufficient that no subject touches a frame edge.** §13g's defect must not return —
   **this correction must not become the next overcorrection**, which is what §13f was.

## 13k. DELIVERED 2026-09-08 — scale and position corrected in CSS, assets untouched

**Delivered at `f2dcc83`.** A `.photo` wrapper clips
`transform: translateY(--panel-shift) scale(--panel-zoom)`; the eight `.webp` files are
byte-identical. **Written in that order the shift is in unscaled screen pixels, so zoom and position
are genuinely independent** — the failure §13f and §13h both had was one control buying two
properties with the trade invisible until it shipped.

**Verified on an independent render:** light subject-height floor 31.3% → 44.3%, dark 41.3% → 64.2%,
dark spread 34.3pt → 19.9pt. **Dark `Start` and `Patches` moved from −6.9 and −3.0 to ≈0 by mass
centroid** — they were the two the before render showed most clearly stranded high.

**Open, and the owner's to judge:** dark `Integrate` and `Grow` remain at −8.9 and −9.4. **The trade is
verified and real** — both subjects already span nearly the full safe frame, so correcting them fully
means reducing zoom below where it started, against §13j.4 rule 1. The implementing round chose
partial correction and disclosed it.

**Still unexplained: §13j.3's light "too high".** The light centroids barely moved; it was tuned by
eye. **If light still reads wrong, no mechanism has been found for it.**

**Recorded for the next report comparing two states: hold one detection threshold across both.** This
round's before and after heights were read at different thresholds, which understated its own
improvement and made the comparison not like-for-like.

## 13l. RULED 2026-09-08 — the "no touching" constraint protects solid geometry, not glow

**The owner narrowed the remaining defect to three panels: light `Grow`, dark `Integrate`, dark
`Grow`. All three must be bigger and sit lower, and light `Grow` must be at least as large as light
`Integrate`.**

**The cause is visible in the shipped values:**

| | light zoom | dark zoom |
|---|---:|---:|
| Start | 1.46 | 1.49 |
| Patches | 1.38 | 1.26 |
| **Integrate** | **1.44** | **1.15** |
| **Grow** | **1.36** | **1.05** |

**Dark `Grow` carries the lowest zoom of all eight, and dark `Integrate` the second lowest** — while
their light counterparts sit at 1.36 and 1.44. **That is exactly why dark 03/04 read smaller than
light**, and light `Grow` at 1.36 reads smaller than light `Integrate` at 1.44 for the same reason.

### 13l.1 What over-constrained them, and it was my rule

§13j.4 rule 4 said *"no subject may touch a frame edge"*, and the implementing round applied it to a
**glow-inclusive** bounding box. **On these two dark panels the halo already spans nearly the whole
frame**, so the rule capped zoom at 1.05-1.15 and left `slack ≈ 0` for any downward shift. The round
reported that trade honestly and it was real **given the rule as written.**

**The rule was wrong. §13g's defect was solid geometry flush against the frame** — a cube whose face
is cut by the edge. **A halo, sparkle, or ground-reflection reaching or crossing the edge is ambient
light, not a cropped object**, and reading it as one is what produced two undersized panels.

**RULED: the constraint protects the solid object. Glow, halo, sparkles and reflection may reach the
frame edge and may be clipped by it.** This is what unlocks zoom *and* downward shift simultaneously
on the two panels where the previous round could get neither.

### 13l.2 Cross-panel consistency is a requirement, not an outcome

**Light `Grow` must be at least as large as light `Integrate`.** The story runs
Start → Patches → Integrate → Grow, and the final panel reading smaller than the one before it works
against the narrative the section exists to tell. **Whatever the measurement says, `Grow` is the
payoff frame.**

### 13l.3 No target numbers, deliberately, for the third time

**I am again not specifying zoom or shift values.** §13h.1's numbers looked right and were rejected;
§13j's were rejected. **The constraint in §13l.1 is the ruling; the values are tuned against the
render and confirmed by the owner.**

## 13m. DELIVERED 2026-09-08 — the three panels, and what the glow ruling actually bought

**Delivered at `bf55412`: three `style` attributes.** dark `Integrate` 1.15 → **1.45** zoom, +4.5 →
**+12px**; light `Grow` 1.36 → **1.48**, −0.7 → **+6px**; dark `Grow` 1.05 → **1.50**, +6.2 → **+16px**.

**§13l.1 was the whole unlock.** The old values were pinned by a margin check protecting *glow*; once
it protected only solid geometry, all three panels had real room for zoom and downward shift together
— the combination the previous round correctly reported it could not achieve under the old rule.

**Verified independently at one held threshold:** light `Grow` 85.1% ≥ `Integrate` 77.6%; dark `Grow`
95.5% ≥ `Integrate` 95.0%. **§13l.2 holds in both themes.**

**Edge check, by a second method:** solid geometry in the dark panels sits at brightness 1.00 and no
border pixel exceeds 0.50. **Nothing solid is cut; only dim glow pooling reaches an edge.**

**Recorded so a later round does not misread it: the headroom is smaller than 95% suggests.** Those
figures include glow. The implementing round stopped when edge closeups stopped looking comfortably
clear rather than when a formula ran out, and dark `Grow`'s solid geometry is now genuinely near its
own margin. **A further increase would cut glow at several edges rather than one.**

**And a caution for whoever measures these panels next**, which cost three rounds to learn: **an
absolute brightness or contrast threshold will classify glow as subject.** The reviewer's own first
pass flagged 119 border pixels as solid geometry on that basis; they were dim amber pooling.
**Compare against the panel's own interior — the panel is its own reference.**

## 13n. FINDING 2026-09-08 — the primary button fails WCAG AA contrast in light theme

**Found while reviewing the 404 page, caused by nothing in it, and more important than it.**

`.button` and the 404's own links are `font-size:.92rem` — **14.72px**, far below WCAG's 18.66px
threshold for "large text". **The requirement is 4.5:1.**

| | text on `--sage` `#7f9179` | ratio |
|---|---|---:|
| **landing, light** — `--btn-ink` `#fffdfa` | | **3.32 — fails AA** |
| landing, dark — `--btn-ink` `#1c201d` | | 4.89 — passes |
| 404, light — `--paper` `#fffdfa` | | 3.32 — inherits it |
| 404, dark — `--paper` `#232823` | | 4.45 — marginally under |

**The first row is `Get started`** — the primary call to action on the project's front page, at a
permanent public URL, in the default theme. **It has been below AA since the button was styled.**

**Five visual rounds and five reviews missed it, mine included**, because every one measured framing,
scale and position and **none measured contrast.** A page can be measured exhaustively along the axes
someone thought to name and still fail an axis nobody did.

**Not ruled here.** The remedy is a palette change — darken `--sage` behind buttons, or use the dark
theme's own dark-on-sage treatment in light theme too (4.89:1) — and it alters the site's most
prominent control. **The owner's call.**

**Worth a sweep rather than a spot fix:** if this control was never contrast-checked, the others
probably were not either — `.chip`, `.nav-cta`, `--ink-soft` body text, and the caption text under
each story panel.

## 9. Scope

**In:** the entrance problem, the truth mechanism, the three-surface division, the measured URL cost,
and §7's five increments.

**Out:** visual design (settled in the drafts review, not re-litigated here); the domain, DNS and
certificate, which are the owner's; any change to `README.md`; any change to `docs/src/index.md`
beyond its unchanged role; and the workspace concept (`010-20260818-01`), which §4.1 records as
claimed-but-not-built and which this RFC does not schedule.
