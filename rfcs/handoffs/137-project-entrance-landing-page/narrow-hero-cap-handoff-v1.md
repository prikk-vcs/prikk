# RFC 137 — the narrow hero cap: the last thing before the domain

> **SUPERSEDED 2026-09-08 by `pre-domain-framing-handoff-v2.md`**, which carries this round's item
> unchanged and adds the story-panel margin fix (RFC 137 §13f). **Work from v2.** This file is kept
> because it was issued and read; nothing in it is wrong, it is just incomplete.

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§13e rules this**, added 2026-09-08
with the acceptance of your third visual pass. **§7.2 is also new; read it, but do not act on it.**
**Base:** `main` at the tip that carries §13e. **Check `git log`, do not trust this line.**
**Prior round:** `.git-exclude/reviewed/137-third-visual-pass-review-v1.md` — accepted, nothing undone.

**One value. This is a short round on purpose:** the owner registers `prikk.org` once the landing page
is ready, and this is the only thing between here and that.

---

## 1. What and why

Your own measurement: residual hero crop **4.04% at 1400px, 0.00% at 1024px, 19.18% at narrow.** You
declined to tune `--hero-cap` to zero one row at the other's expense, and said so rather than picking
silently. **That was right, and the constraint that forced it was mine.**

**§13e released it:** `--hero-cap` was asked for as a *tunable* value, not as one value forbidden a
breakpoint override — and **the hero's aspect ratio is already breakpoint-specific**
(`.hero{aspect-ratio:941/1672}` at narrow). A narrow-specific ceiling is consistent with the structure
already present.

**Why it matters now rather than later:** the domain move makes this page the project's permanent
front door, and **narrow widths are where most first visits land.** Today a phone visitor sees the
worst-framed version of the artwork we ship.

## 2. RULED

**The narrow breakpoint takes its own cap.** Raise it or drop it there; the wide-monitor bound is
untouched, because it exists for a condition narrow widths cannot reach.

At 390px the 9:16 ratio implies a 693px hero, which ordinary phone viewport heights exceed — **so the
narrow crop should go to approximately zero.** If it does not, that is a finding, and report it rather
than tuning toward the number.

**Do not touch the wide cap.** `4.04%` at 1400px and `0.00%` at 1024px are accepted; a change that
improves narrow by moving those is out of scope.

## 3. Controls

Each seen to fail before it passes.

1. **Narrow crop measured from renders**, same technique as last round, at **390px and at a short
   viewport** (≤700px tall) — the second is where a cap re-engages, and one width alone will not show
   it. **Report both numbers.**
2. **The wide numbers are unchanged**: re-measure 1400px and 1024px and show they still read 4.04% and
   0.00%. **This is the control that catches a fix applied too broadly.**
3. **The 2560px case still holds.** Your `width:100%` fix stays correct — re-capture it, because a cap
   change is exactly what could disturb it.
4. **Hero copy still legible at narrow, both themes** — the card, the reveal, and the reduced-motion
   fallback all behave as they did. A taller hero changes what the copy sits on.
5. **Nothing else moved**: story panels, section order, `#start`, and the heading are untouched by this
   diff.

## 4. Explicitly NOT in this round

**Do not begin RFC 137 increment 5 — the `prikk.org` move.** §7.2 now specifies its full scope (four
files plus a new `CNAME`, including `release_notes.rs:52`), **and it is recorded there so it is ready,
not so it can start.** `prikk.org` has no DNS record; §7's ordering constraint is unchanged and neither
half may land early. **It gets its own handoff once the domain resolves.**

**Also not this round:** the `.hero-art` drawing. Its geometry was fixed in the second pass; the
owner's separate objection to its appearance stands and is unspecified. It sits mid-page in `.flow`,
not at the entrance, and it does not gate the domain.

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

`.git-exclude/review-request/`. Keep it short — the round is one value.

- **The two narrow numbers and the two wide ones** (§3.1, §3.2).
- **The cap value you chose**, and whether you raised it or removed it at narrow.
- **Anything that says the page is not ready to be a permanent front door.** You have now built and
  measured this page three rounds running. **If something on it would embarrass the project at
  `prikk.org`, this is the moment to say so** — the owner registers the domain on the strength of it
  being ready, and after that the URL is permanent.
