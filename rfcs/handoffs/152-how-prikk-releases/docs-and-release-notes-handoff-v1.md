# RFC 152 — the documentation and the release notes say how prikk releases

**Live 2026-09-13**, on RFC 152's acceptance. Small; beside RFC 136 increment 1. Nothing here touches
`release-signers.toml`.

1. **`tools/release-policy/src/release_notes.rs::RELEASE_AUTHORITY`** — the paragraph every GitHub
   release carries. It currently says the release *does not pass the DC-35 signer-authority audit*.
   Replace it with RFC 152 §4 in the same length: the tag is signed by the project's one maintainer key
   and verified before it is pushed; checksums and build-info beside every asset prove transport
   integrity; there is no second signer, no support window and no stability promise in v0; verify what
   you obtain by content (`prikk verify`); link the release-compatibility reference. Update the tests
   that pin the paragraph.
2. **`docs/src/reference/release-compatibility.md`** — the sections built on DC-35 (the empty signer set
   as a fault, the two-person classification of disputed tags, the audit) are replaced by RFC 152 §3–§5:
   the procedure, what protects a release, what deliberately does not exist, and what changes when a
   second maintainer arrives. Keep the schema/version-compatibility material; it is unrelated and true.
3. **`docs/src/guide/install.md:30`** and **`docs/src/guide/backup-restore.md:269`** — the sentences
   that say the signer file is "still empty and fail-closed" become RFC 152's statement: one maintainer
   key signs releases in v0; the file is empty because no multi-signer policy exists yet.
4. `SECURITY.md`: read against RFC 152 §4; change only if it contradicts (it should not).
5. Controls: the release-notes generator's tests; `reference-check` (the docs link the RFC); the full gate
   set; no `cfg`. Report: `.git-exclude/review-request/rfc152-docs-and-release-notes-report-v1.md`.
