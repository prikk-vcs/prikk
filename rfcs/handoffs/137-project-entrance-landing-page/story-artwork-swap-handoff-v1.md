# RFC 137 — swap in the replacement story artwork

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§13h rules this round**, and §13g
records why the previous attempt was reverted. **Read both; §13g is a correction to my own ruling and
it explains what not to repeat.**
**Base:** `main` at the tip carrying §13h. **Check `git log`.**
**Source assets:** `.git-exclude/tasks/architect/landing-20260908/prikk-story-image-set-{light,dark}-0{1..4}.png`
— **sources; `.git-exclude/` does not ship.**

**This is the last landing-page item before the owner registers `prikk.org`.** Small, and the CSS box
does not change.

---

## 1. What this is

**Eight replacement panels, one per panel per theme**, closing the artwork item §13g raised. They map
by number: `01` → `Start`, `02` → `Patches`, `03` → `Integrate`, `04` → `Grow`.

**The box stays `aspect-ratio: 1/1`.** §13h.1 computed the trade across six candidate ratios and
square is the best point. **Do not re-litigate the ratio** — but if a render disagrees with the
arithmetic, that is a finding and I want it.

**What changes: the eight `.webp` files, and the eight `object-position` values.** Nothing else.

## 2. Why the old ones failed, so you can check the new ones did not inherit it

Old dark `Integrate` and `Grow` had subjects **larger than the frame** (374px and 427px of a 512px
cell), so they were flush against the edge at any centring. **New margins, measured, normalised to
256:** bottom 35-65 (was 26-63 of 512), left/right 45-63. **Roughly triple, proportionally.**

## 3. Deriving `object-position` — the numbers you need

**Subject extents, normalised to a 256-unit frame** (top margin / bottom margin), so you need not
re-measure:

| | light | dark |
|---|---|---|
| Start | 124 / 60 | 115 / 54 |
| Patches | 93 / 65 | 74 / 60 |
| Integrate | 78 / 53 | 71 / 35 |
| Grow | 71 / 55 | 57 / 42 |

**Source sizes differ by theme** — light ≈598×668, dark ≈1135×1386, and they vary a pixel or two
between files. **Derive per file from its own dimensions; do not assume one size per theme.**

**Expected outcome at square** (§13h.1): six panels centre exactly; **`Start` clamps at +18.6 (light)
and +7.3 (dark)** and that is accepted — its subject is small and low in the source, and it carries
the largest margins of the eight. **Do not chase it.**

## 4. Controls

Each seen to fail before it passes.

1. **Re-measure from the shipped render, not from the source.** Both themes, all four panels, all
   four margins plus off-centre — the same technique §13g used. **This is the control the last two
   rounds each named as skipped**; run it and report the table.
2. **No subject touches a frame edge**, either theme. The two that failed before were dark
   `Integrate` and `Grow`; show them specifically.
3. **Every panel's minimum margin is ≥10% of the frame.** §13h.1 predicts a 12.6% worst case — if
   your render says otherwise, the arithmetic or the crop is wrong and I want to know which.
4. **Six of eight are within ±3 of centre**, and `Start` is the only meaningful exception.
5. **Grid alignment holds** at desktop and the narrow breakpoint — one box shape, one caption baseline.
6. **Byte sizes reported per asset and as a total.** The sources are 139KB-1.4MB PNG; the shipping
   panels were 4-21KB WebP. **Stay in that range.**
7. **Nothing else moved**: section order, `#start`, the four labels, the heading, and the hero.

## 5. Explicitly NOT in this round

- **The box ratio.** Ruled square in §13h.1 against a computed trade.
- **RFC 137 increment 5, the `prikk.org` move.** §7.2 specifies its scope so it is *ready*, not so it
  can start. `prikk.org` still has no DNS record.
- **The `.hero-art` drawing's appearance.** Still open, still not blocking.

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

Landing page, not a shipped crate surface.

## 8. Reporting

`.git-exclude/review-request/`. Keep it short.

- **The measured margin and off-centre table from the render** (§4.1) — this is the report.
- **The `object-position` values you derived**, and the per-file source dimensions you derived them from.
- **Byte sizes.**
- **Whether the render disagrees with §13h.1's arithmetic anywhere**, and by how much.
- **Anything that says the page is still not ready to be a permanent front door.** This is the last
  item before the domain; after it, the URL becomes permanent.
