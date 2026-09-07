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

## 9. Scope

**In:** the entrance problem, the truth mechanism, the three-surface division, the measured URL cost,
and §7's five increments.

**Out:** visual design (settled in the drafts review, not re-litigated here); the domain, DNS and
certificate, which are the owner's; any change to `README.md`; any change to `docs/src/index.md`
beyond its unchanged role; and the workspace concept (`010-20260818-01`), which §4.1 records as
claimed-but-not-built and which this RFC does not schedule.
