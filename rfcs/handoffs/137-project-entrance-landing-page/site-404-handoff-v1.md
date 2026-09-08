# RFC 137 — a 404 page for the site

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§7.2e rules this.** §7.2c finding 3
is what it closes.
**Base:** `main` at the tip carrying §7.2e. **Check `git log`.**

**Small round, one new file.** The domain move is done and verified; this is the last known gap in
the site itself.

---

## 1. The problem

**Any mistyped `prikk.org` URL renders GitHub's generic "Page not found · GitHub Pages".** Confirmed
live. That is the project's own front door failing, reachable from every wrong link anyone writes.

**GitHub Pages serves only the site-root `404.html`.** `docs.yml` stages `docs/landing/` at the
artifact root, so **the file is `docs/landing/404.html`** and **no workflow change is needed** — do
not touch `docs.yml`.

**mdBook's own `book/404.html` stages to `/docs/404.html` and is never served.** Leave it alone; it
is not the mechanism and removing it is not this round's business.

## 2. RULED — the constraint that decides everything else

**A 404 is served at an arbitrary path.** A visitor mistyping `/docs/guide/instal.html` receives this
page **at that URL**, with the browser's base still `/docs/guide/`.

**Therefore every `href` and `src` in it must be root-relative (`/`, `/docs/`) or absolute.** A
relative reference resolves against wherever the miss happened and breaks. **This is the same class
of error the RFC has already been wrong about once**, with `<base>` — get it right by construction and
prove it by fetching the page from a deep path, not the root.

## 3. RULED — its own minimal CSS

**Inline, in the file, duplicating the colour tokens and a little layout. Do not extract a shared
stylesheet.** §7.2's CSS reasoning (inline, 10.6 KiB gzipped) held for one page; extracting one now
would add a request to the landing page's critical path to save bytes on a page almost nobody loads.
**Wrong trade.**

**Take only what you need** from `index.html`'s `:root` block — `--cream`, `--paper`, `--ink`,
`--ink-soft`, `--sage`, `--rule`, the font stack. **Not the whole 21 KB.**

**Accepted cost, already recorded in §7.2e:** these tokens can drift from `index.html`'s. **State in a
comment in the file that a palette change must update both**, so the next person finds it.

## 4. Theme

**Match the rest of the site**: `prefers-color-scheme`, plus the `:root[data-theme="dark"]` /
`:root:not([data-theme="light"])` override shape `index.html` already uses. **Reuse that idiom rather
than inventing a second one** — the landing page has no JS toggle, so `prefers-color-scheme` is what
actually fires, but the override path should behave consistently if anything ever sets it.

**No JavaScript.** The site has none for content decisions and this page must not introduce it.

## 5. Content

**Two routes, because they are the two a lost visitor actually wants:**

- the landing page (`/`)
- the documentation index (`/docs/`)

**Plain and useful. Not clever, not apologetic, not a joke.** Someone arriving here followed a broken
link — most likely one of ours. **The tone is the same as the rest of the site.**

**The wording is yours.** So is whether anything else earns its place; **the burden is on adding, not
omitting.**

## 6. Controls

Each seen to fail before it passes.

1. **Fetch it from a deep path after deploy** — `https://prikk.org/docs/guide/does-not-exist.html` —
   and confirm the page renders **with its styling intact and both links working**. **A root-path test
   proves nothing**: relative references only break at depth, which is exactly why §2 exists.
2. **Both links resolve** from that deep-path context: `/` → 200, `/docs/` → 200.
3. **HTTP status is 404**, not 200. A soft-404 is worse than the generic page for anything reading the
   site programmatically.
4. **Both themes render correctly**, forced explicitly rather than relying on the machine's setting.
5. **No JavaScript** in the file.
6. **Nothing else changed** — `docs.yml` untouched, `index.html` untouched, mdBook's own 404
   untouched.

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

**`reference-check` and the documentation scanners read this file** — a new page under `docs/` may
need declaring the way the landing page was. **If a gate objects, that is information, not an
obstacle.**

## 8. No `CHANGELOG.md` entry

Site infrastructure, not a shipped crate surface.

## 9. Reporting

`.git-exclude/review-request/`.

- **The deep-path fetch result** (§6.1) — this is the control that matters and it is only checkable
  after push, so report the local reasoning and say plainly that it awaits deploy. **The last round
  handled exactly this situation well: do the same.**
- Both themes rendered.
- The byte size.
- **Anything you had to duplicate from `index.html`**, so the drift surface is on the record.
