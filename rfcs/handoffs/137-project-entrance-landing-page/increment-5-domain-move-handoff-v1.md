# RFC 137 increment 5 — move to `prikk.org`

**RFC:** `rfcs/accepted/137-project-entrance-landing-page.md` — **§7.2 is the file list, §7.2a the
meta tags, §7.2b the i18n ruling, §7.2c what measuring the live domain revealed.** Read §7.2c first;
it strikes one requirement §7.2 originally carried.
**Base:** `main` at the tip carrying §7.2c. **Check `git log`.**
**Unblocked:** the owner configured the domain 2026-09-08. **`prikk.org` is live and serving.**

**Do this before 0.37.0.** `homepage` is frozen into each published version, and 0.36.0 shipped
carrying the old URL — landing the move now means no released version straddles the change.

---

## 1. What is already true, measured — do not redo it

- **`prikk.org` resolves to all four Pages addresses and serves `/` and `/docs/`.**
- **The old host 301-redirects to `https://prikk.org/`.** Existing links survive; **no redirect work
  is needed and none should be added.**
- **No `CNAME` file is needed.** §7.2 originally required one; **that was wrong** and §7.2c records
  why. The custom domain lives in repository settings for an Actions-based deploy, and the site
  already serves without one. **Do not add a `CNAME` file.**

## 2. The four files

| file | change |
|---|---|
| `Cargo.toml` | `homepage` → `https://prikk.org/` |
| `docs/book.toml` | `site-url` `/prikk/docs/` → `/docs/` |
| `README.md` | **three** sites — header badge, crate-table badge, Documentation link |
| `tools/release-policy/src/release_notes.rs:52` | the release-compatibility link, **inside every future release's notes** |

**`release_notes.rs` is the one that hides.** It is Rust, not documentation, and it ships in the body
of every release note after this lands. **Verify it by reading the generated notes, not by reading
the diff.**

## 3. Must NOT change

`CHANGELOG.md`, `rfcs/done/129`, the RFC 137 handoffs, and ROADMAP's historical rows. **Those record
what was true at the time.** A sweep-and-replace across the repository will corrupt them — **change
the four files named above and nothing else that mentions the old host.**

## 4. The meta tags (§7.2a)

The landing page has `charset`, `viewport`, `title`, `description`, `icon` and a correct
`<html lang="en">`, **and no social-preview tags at all** — a link shared in chat or on social renders
as a bare URL. **They land here because `og:url` and `og:image` must be absolute**, and doing them
before the move would mean doing them twice.

Add: `og:title`, `og:description`, `og:type`, **`og:url`** (absolute), **`og:image`** (absolute — the
16:9 hero is already a shipped asset and is the obvious candidate), `twitter:card`
(`summary_large_image`) plus whichever of title/description/image it needs,
`<link rel="canonical">` (absolute), and `theme-color`.

**Then fetch the `og:image` URL you wrote and confirm it returns 200.** A card that 404s is worse than
no card, and it is invisible from inside the repository.

## 5. Explicitly NOT in this round

- **No `CNAME` file** (§1).
- **No redirect handling** — GitHub already 301s the old host.
- **No translation** (§7.2b: not until a locale has a named maintainer).
- **The missing site-root `404.html`** — §7.2c finding 3: any mistyped URL at `prikk.org` currently
  renders GitHub's generic page, because the landing site has no 404 of its own. **Real, and it wants
  its own decision rather than being swept in here.** Mention it in your report; do not fix it.
- **`www.prikk.org`'s missing TLS certificate** — owner-side, nothing in this repository touches it.

## 6. Controls

Each seen to fail before it passes.

1. **`/docs/404.html`'s `<base>` reads `/docs/`** after deploy, not `/prikk/docs/`. **Fetch it from
   the live site** — this is the one thing `site-url` actually changes, and it is currently wrong in
   production.
2. **The generated release notes carry the new URL** — produce them via `release-policy` and read the
   body. Not a diff read (§2).
3. **`og:image` returns 200** when fetched at the absolute URL you wrote (§4).
4. **The three `README.md` sites are all updated** — count them; two of the three are badge URLs and
   are easy to miss.
5. **`CHANGELOG.md` and the historical records are untouched** — `git diff` proves it.
6. **The site still serves** `/` and `/docs/` after deploy, and the old host still redirects.

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

**`reference-check` reads documentation links** and this round rewrites several — expect it to have an
opinion.

## 8. `CHANGELOG.md`

**One `## Unreleased` entry**, because this one *is* user-facing: the project's documentation and
homepage move to a permanent URL. Say the old host still redirects.

## 9. Reporting

`.git-exclude/review-request/`. Include:

- **the live `<base>` reading** from `/docs/404.html` after deploy (§6.1);
- **the generated release-notes body** showing the new link (§6.2);
- **the `og:image` fetch result** (§6.3);
- **confirmation the historical records are untouched**;
- **anything else in the repository still naming the old host** that you judged should stay, and why.
  **§3 lists what I know about; you will see the full set.**
