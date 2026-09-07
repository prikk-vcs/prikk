# RFC 137 — the landing page's visual finishing pass

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§10 is the ruling and is settled
input.** §9's exclusion of visual design is lifted for this pass only.
**Base:** `main` at `0c8d965`.
**Origin:** an external visual/UX review commissioned by the project owner. Its diagnosis was
re-derived by the architect and reproduces exactly.

**Read §2 before touching the story section. The obvious fix reintroduces three false claims.**

**Every change here should make the page *smaller*.** The reviewer's own rule, and the owner's: when in
doubt, remove something.

---

## 1. What is wrong, measured

Four things, all verified against `docs/landing/index.html` (436 lines) rather than judged from a
screenshot:

1. **The story strip is ragged and overflows.** `.panels li{width:170px}`, `gap:28px`, over sources of
   `270×170`, `150×310`, `240×300`, `265×400`, `340×400` → rendered heights **~107px to ~351px**, with
   captions at five different heights and no shared baseline. **The desktop overflow is arithmetic:**
   `5×170 + 4×28 = 962px` against a content width of `1000 − 2×28 = 944px`, inside
   `.panels{overflow-x:auto}`. Below 820px it stacks, and stacked it reads as five unrelated pictures.
2. **The hero animation runs backwards.** `@keyframes settle` ends `94%,100%{opacity:0}` on `infinite`:
   **a patch arrives and then vanishes, forever.** The page's own message is that patches are kept.
3. **The accent is absent.** `--clay` appears **once in 436 lines** — `.blk.clay{fill:var(--clay)}`,
   inside the hero SVG. It is nowhere in the page's chrome, so the page reads sage-and-grey while the
   logo and the art are warm.
4. **The terminal clips its own caveats.** The two long `note:` lines in `pre.term` exceed the box and
   are hidden behind `overflow-x:auto`. **The honest disclaimers are the part being cut off.**

## 2. The story section — the fix is not the obvious one

**Do NOT replace the panels with `prikk-attraction-story.png` used whole.** It is the obvious fix, an
external reviewer recommended it, and **it would reintroduce three false claims this RFC exists to have
removed.**

That image bakes a feature strip into its lower edge reading **"Isolated Workspaces — Safe & Parallel"**,
**"Effortless Review — Clear Changes"**, **"Confident Merge"** — which are, verbatim, three of the
claims RFC 137 §4.1 catalogued as false. It also bakes in a headline duplicating the page's own `h1`,
and all of its text is unselectable and inaccessible.

**Using it whole would reinstate as pixels what this RFC removed as prose.** The truth mechanism does
not read images.

### 2.1 What to build instead

**Preferred: one re-rendered image, artwork only.** The five stages on their shared ground line with
the connecting light paths intact, and **no baked text of any kind** — no wordmark, no headline, no
chips, no captions, no feature strip. Chips and captions stay as the real HTML they already are,
rendered beneath the single figure.

**This is what the split was reaching for and missed.** Whole artwork keeps the flow and the glow;
HTML text keeps the truth mechanism and accessibility. Both, not one.

**Fallback if no re-render is available — and it is a good fallback, not a consolation.** Keep the five
committed panels, but as a **CSS grid of five equal cells** with:

- `align-items:end` — one shared baseline, which is what the original artwork had;
- each image in a fixed-ratio frame (`aspect-ratio` + `object-fit:contain`), so unequal sources stop
  producing unequal cells;
- collapsing to an **ordered single column** that keeps the numbered progression, not a gallery.

**Either way `overflow-x:auto` goes.** A horizontal scrollbar at ordinary desktop width is the symptom;
do not fix it by narrowing the panels.

## 3. The hero mark

**Arriving blocks must stay.** Prefer a single gentle settle on load, then static — the page should look
complete when paused, which the direction's §17 requires and the current loop cannot satisfy at any
moment.

**Rebuild the mark as a small, balanced composition echoing the logo's rounded tiles and connecting
pathway**, so the identity lands on the first screen. The current five-rects-and-a-bar sits weighted to
the upper-right of its box and reads as neither a cluster nor a chain.

## 4. Warmth and texture — accepted in its mechanics; the direction is the owner's

**Do these:**

- **Remove most `section{border-top:1px solid var(--rule)}` rules**, and the outlines on
  `.principle-grid`, `.feature-strip`, `pre.term`, `.cmdrow`. Hierarchy comes from spacing and scale.
- **Soft tonal shifts between sections** (alternating `--cream`/`--paper`) in place of hard rules.
- **Use `--clay` deliberately and sparingly** in the chrome — the word "shine", a small mark on the
  primary path. One accent, used once or twice, not sprinkled.
- **One restrained soft-shadow vocabulary**, if any. The direction forbids "excessive shadows".

**The overall aesthetic direction is the project owner's, not yours and not mine.** The reviewer's
framing — *warm the chrome up to meet the art* rather than calm the art down — is endorsed, but the
owner's stated value is **"clean over rich"**. **If a change makes the page feel richer rather than
calmer, stop and show it rather than pressing on.**

## 5. Minor

- **Shorten or wrap the two `note:` lines** so the caveats are readable without horizontal scrolling.
- **Measure dark-mode contrast**: `--ink-soft` on `--paper`, both themes, against **WCAG AA**. This is a
  measurement, not a judgement — report the ratios.
- **Heading cadence** (six sections at one scale) is taste. Optional; skip it if the pass is already
  large.

## 6. What must not be lost — a constraint, not a wish

Verbatim from the reviewer's own "keep it" list. **A styling pass that breaks any of these has failed
regardless of how it looks:**

content truth; the maturity note on the first screen; real terminal output; the copy button and its
live region; **reduced-motion as the opt-in default**; the reflow-friendly command wrap; semantic
sections and heading order.

**Re-check each one after the pass** and say so in the report — do not assume a CSS-only change
preserved them.

## 7. Out of scope

- **Any content change.** The three false claims are gone and must stay gone; do not re-add copy, and do
  not "improve" wording.
- **The three-surface division, the URL shape, `DECLARED_DOCUMENTS`.**
- **Increment 5** (`homepage`/`site-url`) — still DNS-blocked.
- **`README.md`, `docs/src/`.**

## 8. Controls

1. **No horizontal overflow at 1280px, 1024px and 390px.** Measure the story section's rendered width
   against its container; do not eyeball it.
2. **Captions share a baseline** (or there is one image and one caption row).
3. **The hero is complete when paused.** Screenshot it mid-animation and at rest; both must read as
   finished.
4. **The gate still passes.** `docs/landing/` is declared; rule (A)/(B) and `reference-check` must stay
   green — this page is gated, which is the whole reason it lives in this repository.
5. **§6's list re-verified**, item by item.
6. **Dark-mode contrast ratios measured and reported**, both themes.

## 9. Gates

The full set, verbatim from `rfcs/EXECUTION-ORDER.md` §6 rule 9:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo +1.85.0 test --workspace --locked`
- `cargo +1.85.0 check --workspace --all-targets --locked`
- `git diff --check`
- `cargo audit --no-fetch`
- `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`
- release-policy `check`, `boundary-check`, `reference-check`

**`code_regions` scans `<pre>`/`<code>` case-insensitively** (RFC 137 increment 1) — if you touch the
terminal block, run `check` early.

## 10. No `CHANGELOG.md` entry

A styling pass on a web page that ships in no release artifact. **Ruled here rather than left unsaid.**

## 11. Reporting

`.git-exclude/review-request/`. Include:

- **before/after renders** at 1280px and 390px, both colour schemes;
- the measured widths from control 1 and the contrast ratios from control 6;
- **§6's list, re-verified item by item**;
- **whether you took the re-render or the grid fallback** (§2.1), and why;
- **anything that made the page feel richer rather than calmer** (§4) — that is the owner's call, not
  something to resolve quietly.
