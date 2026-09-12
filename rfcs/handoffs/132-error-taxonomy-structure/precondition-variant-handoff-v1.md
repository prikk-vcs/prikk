# Two misleading error prefixes — a `Precondition` variant for the sites a consumer named

**Authority:** `rfcs/done/132-error-taxonomy-structure.md` — increment 1's `#[non_exhaustive]` is what
makes this non-breaking; **increment 2 stays deferred and this is not it.**
**Source:** `.git-exclude/upstream/stikk/receive/001-commit-precondition-error-classes.md`, from the
stikk project (a TUI/GUI front-end driving prikk through the public CLI).
**Review:** `.git-exclude/reviewed/stikk-001-commit-precondition-error-classes-review-v1.md`.
**Base:** `main` at `251f8ba`.

**Scope is two sites and one variant. §5 is the part to read before widening it.**

---

## 1. What is wrong

Two preconditions are reported through whichever `PrikkError` variant was nearest the call site.
Reproduced at `251f8ba` (the reporter observed 0.31.0; both survive 0.32.0):

```console
$ prikk commit --from-worktree --ref heads/other -m "x"
error: lock conflict: active WAL is owned by heads/main; requested ref heads/other

$ prikk commit --from-worktree -m "nothing changed"
error: invalid name: worktree has no node-addressed changes to commit
```

**Both refusals are correct and the active WAL is untouched afterwards.** Only the class prefix is
wrong, and each variant's own doc comment says so:

| Site | Variant | Its documented meaning | What happened |
|---|---|---|---|
| `prikk-store/src/active.rs:227-229` | `LockConflict` | *"A lock could not be acquired because another writer may be active."* | a WAL **ownership** mismatch — nothing is locked, no other process exists |
| `prikk-store/src/worktree_patch/node_authoring.rs:512-515` | `InvalidName` | *"A path-like name failed Prikk path/ref validation."* | an **empty change set** — no name is involved |

**Why it matters beyond tidiness:** RFC 121 ruled exit codes to `0`/`1`/`2`, so `1` covers a refusal,
a dirty worktree and an integrity failure alike. A CLI consumer that must tell them apart has only the
message. The reporter matches `lock` + `conflict`, files case 1 as *transient*, and would offer the
user a lock inspector that shows no lock.

## 2. There is no correct existing variant, and that is the finding

Read every variant in `crates/prikk-error/src/lib.rs:19-56` against these two conditions. **None fits.**
`Integrity` is a stored-object mismatch, `MalformedData` is bad bytes, `Io` is I/O. The category these
belong to is one RFC 132 increment 1 already named when it classified the 29 production sites: **12 of
them are caller-precondition violations**, and increment 2 was expected to give them a variant.

**Add exactly one:**

```rust
/// A caller precondition for the requested operation is not satisfied. Distinct from
/// [`Self::LockConflict`], which means another writer may hold a lock: nothing here is
/// transient and waiting does not help — the caller must change what they asked for.
Precondition(String),
```

with `Display` rendering `"precondition not met: {msg}"`, alongside the existing arms.

**Adding a variant is not breaking.** `PrikkError` is `#[non_exhaustive]` (RFC 132 increment 1,
`264ba73`), so every downstream match already carries a wildcard. That attribute exists precisely so
descriptive variants can be added later, and this is the "later".

**Both sentences are already right and must not change** — only the prefix moves:

```
error: precondition not met: active WAL is owned by heads/main; requested ref heads/other
error: precondition not met: worktree has no node-addressed changes to commit
```

## 3. Why the prefix wording is what it is

The reporter needs to distinguish three things: *wait and retry*, *change your request*, and *the
repository is damaged*. `lock conflict:` and `integrity error:` already carry the first and third.
**`precondition not met:` gives them the second, and says the same thing to a human reading the
terminal.** It also matches this project's own classification vocabulary from increment 1 rather than
inventing a new word for a category we already named.

If you think a different prefix reads better, **report the alternative rather than substituting it** —
this string is now a thing an external consumer will match on, and choosing it is a decision, not a
detail.

## 4. This is a behaviour change, and it must be announced as one

**The reporter classifies on message text and is version-gated.** Changing `lock conflict:` is, for
them, **a breaking change to a string they match** — one they asked for, but still a change their
version gate has to see.

**Write a `CHANGELOG.md` entry under `## Unreleased`** — no version, no date; the release role assigns
those at the cut, and `changelog_headings` ignores a heading that does not split on `" — "`, so an
undated heading is inert to the gate. Say plainly that two error prefixes changed, quote the before and
after, and name the reason. **Do not file it as a typo fix.**

## 5. Do not widen this

**`seal` reaches the same WAL-ownership condition and already presents it correctly**, with no
misleading prefix, because `crates/prikk-cli/src/seal.rs:112-119` returns a plain `String`:

```
error: active WAL is owned by heads/main; requested seal ref is heads/other
```

**Leave it alone.** Routing it through the new variant means routing its four sibling conditions
through variants too — "no patch records to seal", "a damaged record", "ref metadata is
missing/malformed" — or seal's own output becomes internally inconsistent. That is a coherent piece of
work and it belongs with the machine-readable error surface, which is a separate design question the
owner has not opened.

**Also out of scope, explicitly**, because this is *not* RFC 132 increment 2:

- **The other 27 production sites stay where they are.** Two move.
- **`Io { kind }` stays `Option`.** Increment 2 gated narrowing it on the re-classification.
- **No `source()` implementation** — increment 2 measured that at 54 test assertions across 11 files.
- **`Clone` stays.** Measured dead, deliberately kept (RFC 132 §5's closing note).

## 6. Controls

1. **Both commands print the new prefix**, driven through the compiled binary, not asserted from the
   source. The reproduction in §1 is the script.
2. **`LockConflict` still means what it says.** Find its remaining production call sites and confirm
   each is genuinely a lock — if one is not, **report it, do not fix it**; that is a third site and a
   separate decision.
3. **Nothing matches on these variants.** RFC 132 increment 1 measured that the CLI reaches
   `PrikkError` through 126 `map_err(|err| err.to_string())` sites and matches no variant anywhere.
   Re-derive that for the two variants you touch, so "no exit code or control flow changes" is a
   measurement rather than an inherited claim.
4. **Existing tests that assert the old text.** Find them by running the suite, and fix each by
   updating the expectation — **if any test's *intent* was to assert the classification rather than
   the words, say so**, because that is a test that was encoding the defect.

## 7. Gates

The full set from `rfcs/EXECUTION-ORDER.md` §6 rule 9, run as the last action against the exact final
commit: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features --locked
-- -D warnings`; `cargo test --workspace --locked`; `cargo +1.85.0 test --workspace --locked`;
`cargo +1.85.0 check --workspace --all-targets --locked`; `git diff --check`; `cargo audit
--no-fetch`; `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`;
release-policy `check`, `boundary-check`, `reference-check`.

Report the test count per crate that moves. Cross-target clippy only if your own diff introduces
`#[cfg(target_os)]`; check the diff.

## 8. Reporting

Per `.git-exclude/tasks/dev-team/003-landing-work-on-main.md`: commit locally on `main`, do not push,
report to `.git-exclude/review-request/`. Include §6's four control results — particularly control 3's
re-derivation and control 4's list of tests that changed — the changelog entry, and every departure.

**One thing worth knowing about the audience:** the reporter said their classifier degrades to showing
our message verbatim when it recognises nothing, rather than guessing a category. **That is why this
change is safe to make at all.** A message improvement that would break a consumer who guessed would
be a different conversation.
