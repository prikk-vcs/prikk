# RFC 137 — the third visual pass: make the artwork visible

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§13 is new, added 2026-09-08** and
rules this round. **§13b is the owner's own proposal and is the spine of it.**
**Base:** `main` at `4256cb8`. **Check `git log` rather than trusting this line** — the last round
found it stale, correctly.
**Prior round:** `.git-exclude/reviewed/137-second-visual-pass-review-v1.md`. That round was accepted;
nothing in it is being undone.

**The whole round is framing. No new artwork, no re-cropped assets, no new files.**

---

## 1. Why this exists

The four hero images and eight panels are right. **The page is hiding them**, three ways, all measured
in §13a and §13d:

- **31.2% of the hero image height is discarded** by `object-fit:cover` before any scrim applies.
- **The scrim is opaque to 560px of 1400** and fades to 1050px, so only the right quarter is clear —
  and the 16:9 composition puts the cards, which carry the meaning, on the left.
- **At 390px the copy card covers roughly four-fifths** of the 9:16 image; the cube is not visible.
- **Panel subjects sit 51-112px below frame centre in light**, 14-82px in dark.

## 2. The hero reveal — the owner's mechanism, ruled in §13b

**The photograph shows first. The copy fades in over it.**

- **CSS only.** A keyframe animation with a delay. **Do not add JavaScript to this page** — the no-JS
  convention held through the last round and holds here.
- **`prefers-reduced-motion` fallback is copy visible immediately**, unanimated. This is the safe
  state, which is the point: the reveal is a courtesy, never the thing legibility depends on.
- **Same mechanism at both form factors.** The narrow case is the same defect and takes the same fix.
- **No layout shift.** Animate opacity, not position or size. CLS must stay at zero.

**Delay and duration are yours to choose and to justify.** Bounds: long enough that the artwork
registers, short enough that a repeat visitor is not made to wait and that **Largest Contentful Paint
is not materially delayed** — an `opacity:0` heading generally does not count as painted. **Report the
value you chose, the reason, and the measured LCP with and without it.**

## 3. The crop — §13c, RULED

Size the hero so `cover` has little or nothing left to cut. **Size it from the image's aspect ratio,
capped viewport-relatively so it cannot grow absurd on a wide monitor.**

**The hero's own `Get started` may fall below the fold, and that is accepted.** `.nav-cta`
(`index.html:302`) already puts a `Get started` link in the top nav, above the hero, so the first
screen carries the action at every viewport. **The nav is not sticky** — that is fine here, but do not
restate it as though it were. **Do not crop the artwork to protect a duplicate CTA**; that would defeat
the round.

**Mechanism is yours. The property is that the image is substantially whole.** Deliver:

1. The residual crop percentage at **1400px, 1024px and 390px** — measured from renders, not computed
   from the CSS.
2. **Where the `Get started` button falls** relative to a 800px-tall and a 900px-tall viewport, for
   whatever sizing you implement.
3. **The cap you chose and why**, with the height it produces at each of those widths.

**Make the cap a single value** so it can be tuned without restructuring anything.

## 4. The story panels — §13d

**Framing only. The eight `.webp` files are not touched.**

Measured subject centres, per 378×512 cell, frame centre 256 — **use these, do not re-derive them:**

| | Start | Patches | Integrate | Grow |
|---|---:|---:|---:|---:|
| light centre | 368 | 322 | 318 | 307 |
| dark centre | 338 | 296 | 298 | 270 |

- **Give `.panels img` a squarer box** so `cover` has slack to steer; at 3/4 it crops ≈1.6% and there
  is nothing to aim.
- **Aim it with `object-position`.** Per theme at minimum — a value centring light overshoots dark on
  `Grow`. Per panel if that reads better; say which you did and why.
- **The box shape is identical in both themes.** Different shapes break the grid.
- **`Start` will not centre**, and that is expected: §13d records that centring it inside a full-height
  frame needs the frame cut to ≈286px. **Get it to roughly +45 and stop.** Do not crop the asset, do
  not special-case its height, do not chase it.

## 5. Controls

Each seen to fail before it passes.

1. **The reveal degrades correctly.** With `prefers-reduced-motion: reduce` forced, the copy is present
   and legible with no animation, at both form factors and both themes. **Force the media feature
   explicitly; do not rely on the machine's setting.**
2. **The artwork is actually visible before the copy arrives.** A frame captured during the delay shows
   the photograph unobstructed. A screenshot after it does not prove this — capture mid-reveal.
3. **CLS is zero** across the reveal. Measure it; an opacity animation that also moves something is the
   easy mistake.
4. **The residual hero crop is measured at all three viewports** (§3.1), from renders.
5. **Panel subjects are visually centred in both themes**, shown by four panels side by side per theme.
   **`Start` is the known exception and must be stated as such in the report, not quietly passed.**
6. **The grid still aligns** — all four panels share one box shape and one baseline, at desktop and at
   the narrow breakpoint.
7. **Nothing regressed from round 2**: the `#start` anchor still resolves, the four-panel labels are
   unchanged, and the story heading is unchanged.

## 6. Sequencing

1. **Commit 1 — the hero reveal** (§2), no sizing change.
2. **Commit 2 — the hero sizing** (§3).
3. **Commit 3 — the panel framing** (§4).

Each is independently revertable, which matters because §3 carries an owner decision that may come
back the other way.

## 7. Gates

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

## 8. No `CHANGELOG.md` entry

Landing page, not a shipped crate surface. **Ruled here rather than left unsaid.**

## 9. Reporting

`.git-exclude/review-request/`. Include:

- **a mid-reveal capture** (§5.2) — the control that cannot be faked by a finished screenshot;
- **the delay/duration you chose, with LCP measured either way** (§2);
- **the three residual-crop numbers, the CTA fold positions, and the cap you chose** (§3);
- **which `object-position` scheme you used** and why (§4);
- **`Start`'s residual offset**, stated plainly;
- **anything the reveal made worse.** A hero that changes after load is a real risk and you will be the
  first to sit with it. **If it feels like a gimmick rather than a courtesy, say so** — that is worth
  more than a clean report, and the owner would rather hear it now.
