# RFC 137 — three panels: bigger, lower, and the constraint that was stopping you

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§13l rules this round.** Read
§13l.1 first: **it changes the rule that capped your zoom**, and that rule was mine.
**Base:** `main` at the tip carrying §13l. **Check `git log`.**
**Prior round:** `.git-exclude/reviewed/137-story-panel-scale-review-v1.md` — accepted and pushed.
**Nothing you built is being undone.** The mechanism is right; three values are wrong.

---

## 1. Exactly three panels

The owner has narrowed it. **Only these change:**

| panel | theme | now | wanted |
|---|---|---|---|
| `Grow` (04) | **light** | zoom 1.36, shift −0.7px | **bigger** — at least matching `Integrate`'s 1.44 — and **lower** |
| `Integrate` (03) | **dark** | zoom 1.15, shift +4.5px | **bigger** and **lower** |
| `Grow` (04) | **dark** | zoom 1.05, shift +6.2px | **bigger — especially this one** — and **lower** |

**The other five are correct and must not move.** Dark `Start` and `Patches` in particular were
confirmed dead-centre by mass last round; leave them alone.

## 2. Why you could not get there, and the rule that changes

Your report said dark `Integrate` and `Grow` had `slack ≈ 0` — their bounding box already spanned the
safe frame, so more zoom or more shift would touch an edge. **That was true under the rule as I wrote
it, and the rule was wrong.**

**§13j.4 rule 4 said "no subject may touch a frame edge". You applied it to a glow-inclusive bounding
box.** On these two panels the halo spans almost the entire frame, so the rule pinned zoom at 1.05
and 1.15 — **the two lowest values of all eight, against 1.36 and 1.44 on their light counterparts.**
That is precisely why the owner reads dark 03/04 as smaller than light.

**§13l.1 RULES: the constraint protects the solid object only.** §13g's defect was *geometry* cut by
the frame — a cube face sliced off. **A halo, sparkle, or ground-reflection reaching or crossing the
edge is ambient light, not a cropped object.** Let it bleed.

**This is what buys you zoom and downward shift at the same time** on the two panels where you
correctly reported you could have neither.

## 3. Light `Grow` — a consistency requirement, not a measurement

**Light `Grow` must be at least as large as light `Integrate`.** The story runs
Start → Patches → Integrate → Grow; **the final panel reading smaller than the one before it works
against the narrative the section exists to tell.** Whatever any detector says, `Grow` is the payoff
frame. It also needs to come down a little.

## 4. No target values, and this is deliberate

**I am not giving you zoom or shift numbers.** Mine were rejected in §13h.1 and again in §13j.
**§13l.1's constraint is the ruling. The values are yours, tuned against the render.**

**Judge by eye first and let the measurement follow.** Four framing rounds have now been settled by
numbers and three were rejected by the owner looking at them. **If your eye and your detector
disagree, report the eye** — that instruction stood last round and it is stronger this round.

## 5. Controls

Each seen to fail before it passes.

1. **The three panels are visibly bigger and lower**, shown by before/after crops of each.
2. **No solid geometry is cut by a frame edge** on any of the eight. **Glow may be.** State for the
   three changed panels whether glow now reaches the edge, so the ruling's effect is visible.
3. **Light `Grow` ≥ light `Integrate`** in subject scale — report both.
4. **The other five panels are byte-for-byte unchanged in markup** — `git diff` shows exactly three
   changed `style` attributes.
5. **Assets untouched** — `git diff --stat -- docs/landing/assets/` empty.
6. **Grid alignment holds** at desktop and narrow.
7. **One detection threshold, held across before and after.** Last round's tables read "before" at ~40
   and "after" at ~60, which understated your own improvement and made the comparison not
   like-for-like. **Pick one and hold it.**

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

`.git-exclude/review-request/`. **Lead with the renders** — full panel row, both themes, before and
after. The numbers are supporting evidence.

- The three values you chose.
- Whether glow now reaches an edge on any of them (§5.2).
- Light `Grow` against light `Integrate` (§5.3).
- **Whether you think it is right.** You have now looked at these panels more than anyone. **If the
  owner's three items are fixed but something else has moved, say so** — this is the fifth framing
  round and each one has been narrowed by someone looking rather than measuring.
