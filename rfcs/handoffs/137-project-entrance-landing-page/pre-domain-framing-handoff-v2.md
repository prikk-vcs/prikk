# RFC 137 — framing fixes before the domain: the narrow hero cap, and the story panels' margins

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§13e and §13f rule this round.**
**§7.2 is present and must not be acted on.**
**Base:** `main` at the tip carrying §13f. **Check `git log`; do not trust this line.**
**Supersedes:** `narrow-hero-cap-handoff-v1.md`, issued minutes earlier. Item 1 below is unchanged from
it; item 2 is new, and both are landing-page framing, so they belong in one round rather than two.

**Both items must land before the owner registers `prikk.org`** — after that this page is the
project's permanent front door.

---

## 1. The narrow hero cap (§13e) — unchanged from v1

Your measurement: residual hero crop **4.04% at 1400px, 0.00% at 1024px, 19.18% at narrow.** You
declined to tune `--hero-cap` to zero one row at the other's expense. **That was right, and the
constraint that forced it was mine.**

**RULED: the narrow breakpoint takes its own cap.** The hero's aspect ratio is already
breakpoint-specific (`.hero{aspect-ratio:941/1672}`), so a narrow-specific ceiling is consistent with
what is there. At 390px the 9:16 ratio implies a 693px hero, which ordinary phone viewports exceed —
**narrow crop should go to approximately zero.** If it does not, report it rather than tuning toward
the number.

**Do not touch the wide cap.** 4.04% and 0.00% are accepted; a change that improves narrow by moving
those is out of scope.

## 2. The story panels' margins (§13f) — new, and the diagnosis is a correction to my own ruling

**The owner reports the images are too tight on all four sides, dark worse.** Confirmed by measurement;
the full table is in §13f. The sharp end: **dark `Integrate` is 1/0 and dark `Grow` is 0/0** — subject
flush against the frame, top and bottom.

**The cause is §13d, which is mine.** Squaring the box to create `object-position` slack spends 134px
of the 512px source height on crop, **and that crop is the margin the artwork carries.** Source bottom
margins are only 26-63px, so a 134px crop removes them outright. **Centring and margin come from one
budget. I bought the first with the second and did not say so.**

**RULED: margin wins.** A subject flush against a frame edge reads as a mistake; a subject slightly off
centre does not. **Reduce the crop until every panel shows visible margin on all four sides in both
themes, and take whatever centring the remaining slack allows.**

**The ratio is yours to choose, against measurement.** It is a continuous trade — every pixel of crop
returned costs a pixel of centring slack. **Do not argue it in advance; measure two or three
candidates and show the trade.** §13d's per-theme `object-position` structure stays; only its budget
changes.

**Source margins, so you need not re-derive them** (top/bottom, per 378×512 cell):

| | light | dark |
|---|---|---|
| Start | 273 / 47 | 219 / 53 |
| Patches | 195 / 62 | 144 / 63 |
| Integrate | 156 / 31 | 111 / 26 |
| Grow | 137 / 34 | **57 / 27** |

**`Grow` dark is the hard case** — 57/27 in the source means even zero crop leaves it modest. **If it
cannot reach your target, say so and stop.** That is artwork, not CSS, and it is the owner's call, the
same category as `Start`'s residual offset.

**Left/right matters too** — light `Patches` right is 6px and dark `Patches` right is 8px. Whatever you
change, **report all four sides**, not just the two the crop moves.

## 3. Controls

Each seen to fail before it passes.

1. **No subject touches a frame edge, either theme.** The failing case to reproduce first is dark
   `Integrate` and dark `Grow` at 0-1px — show them failing, then passing.
2. **All four margins reported per panel per theme**, measured from renders in the shipped image box,
   the same technique §13f used.
3. **Residual centring reported alongside**, so the trade is visible. **A margin fix that quietly
   ruins centring is not an improvement; both numbers together are the result.**
4. **Narrow hero crop at 390px and at a short viewport (≤700px tall)** — one width alone will not show
   a cap re-engaging.
5. **The wide hero numbers are unchanged** — re-measure 1400px and 1024px and show 4.04% / 0.00%. This
   catches a cap fix applied too broadly.
6. **2560px still holds** — the `width:100%` fix stays correct; re-capture it, since a cap change is
   what could disturb it.
7. **Nothing else moved**: section order, `#start`, the four labels, and the heading are untouched.

## 4. Explicitly NOT in this round

**Do not begin RFC 137 increment 5 — the `prikk.org` move.** §7.2 specifies its full scope (four files
plus a new `CNAME`, including `release_notes.rs:52`) **so it is ready, not so it can start.**
`prikk.org` has no DNS record; §7's ordering constraint stands and neither half may land early.

**Also not this round:** the `.hero-art` drawing's appearance, and any change to the eight `.webp`
files. **This round is CSS.**

## 5. Gates

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

## 6. No `CHANGELOG.md` entry

Landing page, not a shipped crate surface.

## 7. Reporting

`.git-exclude/review-request/`.

- **The margin table and the centring table, side by side** (§3.2, §3.3) — the trade is the finding.
- **The candidate ratios you measured** and why you chose the one you did.
- **`Grow` dark's result**, stated plainly, and whether you judge it needs artwork.
- **The hero numbers** (§3.4, §3.5, §3.6) and the cap value you chose.
- **Anything that says the page is not ready to be a permanent front door.** You have built and
  measured this page four rounds running. **If something on it would embarrass the project at
  `prikk.org`, this is the moment** — the owner registers on the strength of it being ready, and the
  URL is permanent afterwards.
