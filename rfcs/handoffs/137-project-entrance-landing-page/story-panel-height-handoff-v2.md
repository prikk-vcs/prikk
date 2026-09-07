# RFC 137 §2 — required follow-up: the image box is 420px tall, not square

**Follows:** `visual-finishing-pass-handoff-v1.md` §2/§2.2a, **ACCEPTED and pushed** (`6055149`) with
one required follow-up.
**Base:** `main` at `6055149`.

**This is one line of CSS. The round is otherwise accepted in full.**

---

## 1. What is wrong

`.panels img` sets **no height**:

```css
.panels img{display:block;width:100%;aspect-ratio:1/1;object-fit:contain}
```

The markup carries `height="420"` as a presentational hint. **With no author height, that hint
applies** — and **`aspect-ratio` only fills a dimension that is `auto`.** Height is 420px, so
`aspect-ratio:1/1` is ignored entirely.

**Result: the box is 172.8 wide × 420 tall**, and `object-fit:contain` letterboxes the artwork into the
middle of it.

## 2. The consequence, measured from your own render

Content bands across `after-1280-light.png`:

```
chips     y=184..207
artwork   y=375..523    (148px of visible artwork)
captions  y=658..694
```

**Each panel reserves ~425px of vertical space to show 148px of artwork** — roughly **300px of dead
space per panel**, which is the large void above and below the row.

**Your own report contains the evidence:** `imgTop=2226.30`, `capTop=2660.30` — a **434px** gap, where a
1:1 image in a 172.8px column would give ~187px. **The measurement was right; what it implied was
missed.** Worth carrying: a number that does not match the model should be reconciled, not recorded.

## 3. The fix

Add `height:auto` to `.panels img`.

**Keep the `width`/`height` attributes in the markup** — they are correct practice for layout stability
before the image loads, and they are not the problem. The problem is that CSS never overrode the
height, so `aspect-ratio` had nothing to fill.

**Do not remove `aspect-ratio`** — with `height:auto` it becomes the thing that actually governs the
box, and it documents the intent.

**Expect the section to lose roughly 250px.** That is the right direction: every change in this work
should make the page smaller.

## 4. Out of scope

- **The crops.** All five are correct — one ratio, one scale, nothing severed, verified by bounding-box
  measurement. Do not re-cut them.
- **The grid, the removed `align-items:end`, `.story`'s fixed background.** All correct.
- Everything else already ruled out of scope in v1 §7.

## 5. Controls

1. **The image box is square.** Measure the rendered `<img>` box at 1280px: width and height equal, and
   both ≈ the column width. Do not infer it from the CSS.
2. **The void is gone.** Re-measure the three bands from §2 and report them. The gap between the
   artwork's bottom and the caption's top should be the caption margin, not ~135px.
3. **Alignment survives.** Chips, images and captions still share their positions across all five
   columns — the property the last round established must not regress.
4. **No overflow at 1280px, 1024px, 390px**, measured as before.

## 6. Gates

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

## 7. No `CHANGELOG.md` entry

Visual-only, no user-facing behaviour change — the standing ruling for this work.

## 8. Reporting

`.git-exclude/review-request/`. Short is correct. Include the four control measurements and a
before/after render at 1280px showing the section's reduced height.
