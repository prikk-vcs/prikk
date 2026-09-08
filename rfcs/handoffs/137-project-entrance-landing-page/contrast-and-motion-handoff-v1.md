# RFC 137 — fix the light-theme contrast failures, and add modest motion

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§13o rules the contrast, §13p the
motion.** §13n is how it was found.
**Base:** `main` at the tip carrying §13o/§13p. **Check `git log`.**

**Two jobs, one round, but they are independent** — keep them in separate commits so either can be
reverted without the other.

---

## Part 1 — contrast

### 1.1 What is wrong, measured

Every text-on-background pair on the page, both themes, against WCAG AA. **Nothing here is "large
text"** — the controls are `.92rem` = 14.72px, far under the 18.66px threshold — so **4.5:1 applies
everywhere**.

**Dark theme passes all fifteen pairs, lowest 4.89. Do not touch it.**

**Light theme fails three:**

| | | ratio |
|---|---|---:|
| `.eyebrow` — `--sage-dim` on `--cream` | `#93a48d` on `#f8f3ec` | **2.40** |
| primary button — `--btn-ink` on `--sage` | `#fffdfa` on `#7f9179` | **3.32** |
| terminal prompt `.p` — `--sage` on `--paper` | `#7f9179` on `#fffdfa` | **3.32** |

### 1.2 RULED — a constraint on light `--sage`, not a colour

**Light `--sage` must satisfy both**: white text on it ≥ 4.5, and it as text on cream ≥ 4.5.
**Relative luminance ≤ ≈0.145**, against today's 0.2614.

**One token change fixes the button and the terminal prompt together.** `#5f6e5b` sits on the boundary
(5.35 / 4.92) — **that is arithmetic, not a recommendation.** Pick the colour inside the constraint;
the choice is yours and the site's, and going somewhat darker buys margin.

**Report the ratios you land on**, not just the hex.

### 1.3 `.eyebrow` needs its own answer, and it is the awkward one

`--sage-dim` is luminance 0.3468 — nowhere near the constraint — and at **2.40 it is the worst pair on
the page**, appearing above every section heading.

**Darkening `--sage-dim` far enough to pass would make it darker than `--sage`**, inverting what its
name asserts. **So this is a decision, not a tweak:** either the eyebrow stops using `--sage-dim`
(`--ink-soft` already passes at 5.49 and is used for every other quiet text on the page), or the
sage/sage-dim pair is re-derived so the relationship still holds.

**Say which you chose and why.** If you re-derive the pair, check every other `--sage-dim` use.

### 1.4 Do not "fix" dark theme

Its `--sage` is `#93a48d` and serves as a **foreground**; light's `#7f9179` is a **background**. **The
token plays opposite roles per theme and only light is broken.** A symmetrical change would break a
theme that currently passes.

### 1.5 The 404 inherits this

`docs/landing/404.html` duplicates these tokens by hand (§7.2e's accepted cost). **Whatever you change
in `index.html`, mirror it there** — the file's own top comment says so, and this is the first time it
is being tested.

## Part 2 — motion

### 2.1 What exists

**Two `@keyframes` and not one `transition:` declaration.** Every hover state on the page changes
instantly — the primary button, nav links, the copy button, the 404's links. **That is the gap.**

### 2.2 RULED — two bounds, non-negotiable

1. **`prefers-reduced-motion: reduce` falls back to the finished state**, never a hidden or
   half-rendered one. §13b is the precedent: content fading *in* is safe because the fallback is
   *content visible*. **Any reveal you add must pass that test.**
2. **No JavaScript.** The page has none for content decisions. **This rules out scroll-triggered
   reveals** — `IntersectionObserver` is JS and CSS scroll-driven animation is not broadly available.
   **On-load or hover only.**

**Also bounded:** animate colour, background, border and opacity — **never layout, never size,
never position.** CLS must stay zero and you must measure it, not assume it.

**Reuse the house timing** — `.8s cubic-bezier(.16,1,.3,1)` — rather than introducing a second motion
vocabulary. Hover feedback wants to be faster than a reveal; **pick a shorter duration in the same
curve rather than a different curve.**

### 2.3 What to do, and the judgement is yours

**Hover feedback on the existing hover states is the clear win** — those states already exist and
change instantly today. **Cheap, felt immediately, and it costs nothing on load.**

**A card fade-in on load is permitted but be sparing.** The hero already delays its own copy 500ms;
**more on-load reveals compound into a page that assembles itself while the reader waits.** If you add
one, say why it earns the delay.

**"Modest" is the owner's word and it is the specification.** If you find yourself adding a third
kind of motion, that is the signal to stop.

## 3. Controls

Each seen to fail before it passes.

1. **Every pair in §1.1 re-measured after the change**, both themes, and reported as ratios. **All
   fifteen light pairs, not only the three that failed** — a token change touches everything using it.
2. **Dark theme's fifteen are unchanged**, proving §1.4 was respected.
3. **The 404's duplicated tokens match** `index.html`'s new values (§1.5).
4. **`prefers-reduced-motion: reduce` forced explicitly** — every animated element is in its finished
   state, nothing hidden, at both themes.
5. **CLS is zero** across every animation you add. Measure it.
6. **Hover states still work without motion** — a hover that only communicates through a transition
   fails for reduced-motion users. **The end state must differ visibly, not just the path to it.**
7. **Nothing else moved** — no layout, no content, no assets.

## 4. Gates

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

## 5. No `CHANGELOG.md` entry

Landing page, not a shipped crate surface.

## 6. Reporting

`.git-exclude/review-request/`. Two commits, reported separately.

**Contrast:**
- the full ratio table, both themes, after the change (§3.1, §3.2);
- **what you did about `.eyebrow`** and why (§1.3) — this is the interesting decision;
- confirmation the 404 was mirrored.

**Motion:**
- what you added, and the durations;
- **the reduced-motion render** (§3.4) and the CLS measurement (§3.5);
- **whether any hover still communicates without its transition** (§3.6).

**And say if the contrast fix made the page look worse.** A darker sage is a real visual change to the
site's accent colour. **If it costs something, the owner should hear it from you rather than notice it
later** — accessibility wins the argument, but the cost should be stated, not hidden.
