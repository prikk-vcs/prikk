# RFC 137 — the second visual pass: four-panel story, photographic hero, reordered page

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§11 is new, added 2026-09-08** and
rules this round. **Read §11d before you touch the story CSS**; it is the defect the last two rounds
both hit.
**Base:** `main` at `e589923`.
**Source assets:** `.git-exclude/tasks/architect/landing-20260908/` — **sources, not shippable.**
`.git-exclude/` does not survive a clone; converted assets go in `docs/landing/assets/`.

---

## 1. What the owner supplied, and what is already settled

Four hero images (16:9 and 9:16, each light and dark) and one story sheet carrying **four** panels in
**both** themes. §11a-§11g rule the parts that were open. **Three things are decided and are not
yours to re-open:** the section order (§11a), the fourth panel's label (§11b), and WebP (§11g).

**One thing is the owner's and is not yet answered** — the `.story` heading (§11b). Build with the
current text and **flag it in your report**; do not invent a replacement.

## 2. Section reorder — move whole, invent nothing

Current DOM order → required order:

```
hero  principles  flow  terminal-section  story  features  install  closing
hero  install     flow  principles        story  features  terminal-section  closing
```

**`.install` keeps `id="start"`** — the hero's own `Get started` button targets it, and the anchor
must not break. It simply travels much less far now.

**Move the `<section>` blocks intact.** No content edits in the same commit as the move (RFC 131 §5's
rule, and it applies to markup for the same reason): a move that also edits is unreviewable.

## 3. The story panels

### 3.1 Crop geometry, measured — do not re-derive it

`prikk-story-image-set-01-light-dark-compat.png`, 1536×1024, cells exactly **384×512**:

| | x | y |
|---|---:|---:|
| columns | 0, 384, 768, 1152 | — |
| light row | — | 0 |
| dark row | — | 512 |

**Each column boundary carries a ≈2px near-white seam.** Cropping on the nominal edge drags it into
the panel. **Inset horizontally** — 378 wide at `x+3` is the straightforward reading; verify by eye at
the joins rather than trusting the number.

The row split at `y=512` is a hard transition with no seam.

### 3.2 Eight files, four names

`Start`, `Patches`, `Integrate`, `Grow` — **"Grow", not "Shining" and not "Grow and Shine"** (§11b).
Two themes each. Follow the existing `panel-N-name.webp` convention and extend it for theme; the
current five `panel-*.webp` files are replaced, not kept alongside.

### 3.3 The CSS, and the trap

- `.panels{grid-template-columns:repeat(5,1fr)}` → **`repeat(4,1fr)`**
- `.panels img{aspect-ratio:1/1}` → **`3/4`** — the new cells are 384×512
- The `@media` block at the narrow breakpoint sets `grid-template-columns:1fr` and
  `max-width:260px`; **re-check that 260px still suits a 3:4 panel**, since it was chosen for a square

**§10's own round found `aspect-ratio` silently not applying and the fix was `height:auto`.** Do not
assume it applies this time because the rule is present. **Measure a rendered panel and put the
measured height in your report.**

### 3.4 Theme switching

The page already switches theme by `prefers-color-scheme` **and** `[data-theme]` (`index.html:18-25`).
The panels must follow **both**, the same way the rest of the page does. `<picture>` with a
`prefers-color-scheme` source alone will not answer a `[data-theme]` override — **whatever you choose,
demonstrate it switching by explicit override, not only by OS setting.**

## 4. `.story`'s background — RULED, and it is the round's real hazard

`.story{background:#fdf6ee}` is hardcoded, pinned to light in §10.1c because the old panels could not
theme-adapt. **That pin comes off, and the dark value is `#1d2a28` — sampled from the artwork, NOT
`--paper` (`#232823`).**

**Using the site token would leave every dark panel sitting on the page as a visible rectangle**, a
slightly-wrong dark green against the artwork's own. That is precisely the defect class of §10.1a and
§10.1d. **The light value `#fdf6ee` is already correct against the artwork's `#fdf6ef`; leave it, and
do not "tidy" either value into a token.**

## 5. The hero

Four images, two axes: form factor (16:9 desktop / 9:16 mobile) and theme.

**Know what you are shipping (§11c):** the 16:9 light is **daylight**; the 9:16 light is **dusk**, and
pairs with the 9:16 dark rather than with its own theme-mate. **This is the owner's to resolve and is
not yours to fix by filtering or re-grading.** Ship the four as given, and **state the discrepancy in
your report** so it is on the record at the point someone looks at the staged page.

**The composition difference between form factors is correct** — cards flow horizontally into the cube
at 16:9 and descend onto it at 9:16. It is a re-composition, not a crop. **Do not implement the
mobile case by cropping the desktop image.**

**None of the four carries baked-in text.** Keep it that way: the hero copy stays live DOM (`.eyebrow`,
`<h1>`, `.lead`, `.maturity`, `.cta`) over the image. It must remain selectable, translatable, and
legible at the narrow breakpoint. **Check contrast of the live text against the image at both themes**
— the 16:9 light is bright, and dark text over a bright sky is the easy half; light text over dusk is
where this usually fails.

## 6. `.hero-art` — re-draw, then move; do not delete

The owner ruled it moved rather than removed, **after** a re-draw. §11f locates the defect:

- Left connector `M46 108 V82 H114 V66` rises clear of the blocks, travels in open space, enters the
  upper block, and has a node at its bend `(114,82)`.
- Right connector `M182 108 H250` runs at `y=108`, which is **the top edge of both blocks** — it skims
  along them for 26px each side instead of crossing open space, and **has no node at its bend.**

**Mirror the left: `M182 108 V82 H250 V66`, with a node at `(250,82)`.** No collision — the left path
occupies `y=82` only from `x=46` to `x=114`.

**Where it moves is yours to propose, not to decide silently.** The owner said "somewhere", which is
an instruction to suggest one. Say where and why in your report; do not land it in a new location as
a fait accompli.

## 7. WebP — mandatory

The four hero PNGs are 1.6-2.0 MB each; the set is **≈7 MB**. The panels already shipping are WebP at
2-12 KB. **Convert everything.** Report the delivered byte size per asset and the page total — a
landing page whose entrance image outweighs the rest of the site defeats §1.

**Keep the PNG sources where they are.** `.git-exclude/` does not ship.

## 8. Controls

Each seen to fail before it passes.

1. **Both themes, both form factors — four screenshots**, and the story panels' background is
   indistinguishable from the section's at both themes. **Deliberately set the dark story background
   to `--paper` once and confirm the seam becomes visible** — that is the control for §4, and if you
   cannot see the difference, say so and show the measurement.
2. **Theme switches by `[data-theme]` override, not only by OS preference.**
3. **The rendered panel height is measured, not assumed** (§3.3), at desktop and at the narrow
   breakpoint.
4. **The `#start` anchor still resolves** from the hero button after the reorder.
5. **No section's content changed in the move commit** — `git diff` on that commit shows only
   relocation.
6. **Hero text is legible at the narrow breakpoint over both theme images**, checked at an actual
   narrow viewport rather than by shrinking a desktop screenshot.

## 9. Sequencing

1. **Commit 1 — the reorder only.** No asset work, no CSS.
2. **Commit 2 — assets and the CSS that serves them** (panels, backgrounds, hero).
3. **Commit 3 — the `.hero-art` re-draw and its relocation.**

Stopping after 1 or 2 leaves a coherent page; stopping inside one does not.

## 10. Gates

`docs/landing/` is not Rust and most of the workspace set is inert here, but **run the full set
anyway** — the reference and command scanners read documentation, and this round moves a lot of it:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo +1.85.0 test --workspace --locked`
- `cargo +1.85.0 check --workspace --all-targets --locked`
- `git diff --check`
- `cargo audit --no-fetch`
- `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`
- release-policy `check`, `boundary-check`, `reference-check`

## 11. No `CHANGELOG.md` entry

The landing page is not a shipped crate surface. **Ruled here rather than left unsaid.**

## 12. Reporting

`.git-exclude/review-request/`. Include:

- **the four screenshots** (§8.1) and the measured panel height (§8.3);
- **the byte size of every delivered asset and the page total** (§7);
- **your proposed home for the re-drawn `.hero-art`**, with the reason (§6);
- **confirmation that you saw the two-times-of-day discrepancy** in the light heroes (§5), stated in
  your own words — it is the owner's to resolve, and this report is where it stays visible;
- **anything the reorder made read worse.** You will be the first to see the page in the new order,
  and §11a names one candidate already: the hero warns the reader off, then position 2 asks them to
  install.
