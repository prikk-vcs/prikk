# RFC 147 §3f — Case C: an unrepresentable name says `authored` while `commit` refuses it

**Live 2026-09-13**, on stikk's letter 010 (measured by them on 0.41.0 and 0.28.0; reproduced by the
architect on the tree). Small; one commit; fits before or beside RFC 151's follow-up.

## 1. The two facts

`worktree_status.rs` (~line 339), the `Err` arm of `path_to_repo_string(root, &path).and_then(RepoPath::parse)`:

```rust
changes.push(WorktreeChange {
    path: path.display().to_string(),          // the absolute OS path
    kind: WorktreeChangeKind::UnsupportedPath,
    detail: format!("worktree path is not representable as a safe Prikk path: {err}"),
    refusal: None,                             // so `authoring: "authored"`, `refused_count` unchanged
});
```

`commit` on the same tree: `error: invalid name: backslashes are not allowed in repository paths` — the
very `err` this arm holds and discards. RFC 147 §2e(b) kept `unsupported-path` meaning "unrepresentable
name" and was right to; it left the entry's *verdict* wrong. Same gap as Case A, second place.

## 2. The change

1. **`refusal: Some(<the text commit prints>)`** — rendered through the same error Display `commit`
   uses, so the string is identical (stikk relays it verbatim). `authoring` becomes `"refused"`,
   `refused_count` counts it, prose prints the refusal line as for any refused entry. The `kind` stays
   `unsupported-path`; the `detail` stays.
2. **`path` becomes the OS name rendered relative to the worktree root, lossily** (strip `root`,
   `to_string_lossy`), not the absolute path — an absolute filesystem path in a report a front-end shows
   is the machine's layout leaking into a repository-scoped document, and no repo-relative form exists
   only in the strict sense. **State it in `worktree-status-report-v1`'s description** (schema doc and
   `commands.md`/`worktree-status` docs): *`path` is repo-relative for every kind except
   `unsupported-path`, where it is the OS name relative to the worktree root, rendered lossily.*
   Field addition/precision within `-v1` is the established precedent; this is a value change for one
   kind that no consumer displayed (stikk never rendered these entries) — say so in the CHANGELOG.

## 3. Controls

- A tree with `back\slash.txt` (Unix) and a non-UTF-8 name (Unix; skip on Windows with the reason):
  `worktree-status --format json` entry has `kind: "unsupported-path"`, `authoring: "refused"`,
  `refusal` **byte-equal** to `commit`'s first stderr line minus `error: ` (the RFC 147 agreement-test
  shape), `refused_count: 1`; `path` starts with no `/` and equals the root-relative name.
- The agreement test that already drives both commands over one fixture gains these two rows.
- Perturb: `refusal: None` restored → the agreement test fails on `authored`; the absolute path restored →
  the `path` assertion fails.
- Full gate set; the addendum applies (`path.rs` carries platform gating for UTF-8/OS strings — run both).
- CHANGELOG `### Fixed` (the verdict) and `### Changed` (the path value), naming stikk's letter 010.
- Report: `.git-exclude/review-request/rfc147-unrepresentable-names-report-v1.md`.
