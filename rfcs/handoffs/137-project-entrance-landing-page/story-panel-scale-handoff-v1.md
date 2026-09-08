# RFC 137 — the story panels are too small and sit too high

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§13j rules this round.** Read it
first: it is a correction to my own §13h, and it says which of my numbers to stop trusting.
**Base:** `main` at the tip carrying §13j. **Check `git log`.**
**Prior round:** `.git-exclude/reviewed/137-story-artwork-swap-review-v1.md` — accepted, and the
measurement in it was correct about the wrong quantity. **Nothing you delivered is being undone; the
assets stay.**

---

## 1. What is wrong, in the owner's words

**"All the images are too small (although the margin too large), which lacks visual impact. Also the
content vertical position is too high — a little on light theme and seriously on dark theme."**

**Both are real. §13h.1's targets were met and the result still looks wrong**, because the targets did
not include the two things that matter here.

## 2. Scale — the axis nobody measured

Subject height as a fraction of the 201px frame, from the shipped render:

| | Start | Patches | Integrate | Grow |
|---|---:|---:|---:|---:|
| light | **30.8%** | 42.3% | 53.7% | 57.2% |
| dark | 41.3% | 57.7% | 70.6% | **75.6%** |

**Two problems in one table.** The subjects are small, and **the set is wildly inconsistent** —
`Start` at 30.8% next to `Grow` at 57.2% in the same row, and light systematically smaller than dark.

## 3. Position — and why my previous numbers said "centred"

**A bounding box is not perceived mass.** On dark, the glow and ground-reflection below the subject
land inside the bounding box and count for nothing to the eye:

| | bbox centre | mass centroid |
|---|---:|---:|
| dark Start | +6.5px | **−7.1px** |
| dark Integrate | −1.0px | **−9.7px** |
| dark Grow | −1.0px | **−12.2px** |

**The box is centred while the visual weight sits up to 13.6px high on a 201px frame.** That is the
"seriously on dark".

**On light I cannot reproduce the owner's reading** — there the centroid sits *below* the box centre.
**§13j.3 records that I have no mechanism for it.** Tune light by eye against the render; do not go
looking for a number that explains it, and do not assume my dark explanation transfers.

## 4. RULED

1. **Zoom in CSS. Do not re-crop or replace the eight `.webp` files.** The artwork is right; the
   framing is wrong. Keeping the assets intact keeps this reversible and cheap to re-tune — which
   matters, because this is the second time framing has needed adjusting.
2. **The subject must dominate its frame, and the set must be visually consistent across all eight.**
   **I am deliberately not giving you a target percentage.** §13h.1's failure was a number that
   looked right and produced a page the owner rejected; a second number from me would carry the same
   risk. **Tune it against the render.**
3. **Vertical position is tuned by mass, not bounding box** — §3's centroid is the better instrument.
4. **No subject may touch a frame edge, either theme.** §13g's defect must not return. **This
   correction must not become the next overcorrection** — that is exactly what §13f was, and it cost
   a full round.

## 5. Mechanism — yours, with one constraint

A scale plus a vertical offset per panel per theme is the obvious shape, and `object-position` alone
cannot zoom. **Whatever you choose, the two knobs must be independent** — the last two rounds both
failed because one control was being used to buy two properties, and the trade was invisible until it
shipped.

**Mind the rounded corner.** `border-radius` currently sits on the `<img>`; a zoom that overflows its
frame needs the clip and the radius to end up on whichever element actually bounds the image.

## 6. Controls

Each seen to fail before it passes.

1. **Subject height per panel per theme, from the render**, before and after. **The spread across the
   set is the number that matters most** — report it explicitly, not just the eight values.
2. **Mass centroid per panel per theme**, not bounding-box centre (§3). **Report both**, so the gap
   between them stays visible to whoever reads this next.
3. **No subject touches a frame edge**, either theme, all eight.
4. **Grid alignment holds** at desktop and the narrow breakpoint.
5. **The assets are byte-identical** to what shipped — `git diff` shows no change under
   `docs/landing/assets/`.
6. **Nothing else moved**: section order, `#start`, the four labels, the heading, the hero.

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

Landing page, not a shipped crate surface.

## 9. Reporting

`.git-exclude/review-request/`. Include the before/after tables from §6.1 and §6.2, the values you
chose, and **full-width renders of the panel row in both themes** — the owner is judging appearance,
so the render is the deliverable and the numbers are supporting evidence, not the other way round.

**And say if you think it still looks wrong.** Four framing rounds have now been decided by
measurement and two of them were rejected by eye. **If your own eye disagrees with your numbers, the
eye is the one to report.**
