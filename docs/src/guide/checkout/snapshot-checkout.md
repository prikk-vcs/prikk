# Snapshot Checkout Planning

PR-017 keeps a read-only snapshot checkout planning path.

The command:

```sh
prikk checkout --snapshot-plan [path] [--ref <ref|block-id>]
```

validates the current ref, target block, snapshot Blob object, and snapshot manifest paths. It does
not write the worktree.

The current path-safety scaffold is deliberately conservative. It rejects:

- absolute paths
- `.` and `..` components
- empty components
- backslashes and colon characters
- control characters
- Windows reserved names such as `CON`, `NUL`, `COM1`, and `LPT1`
- characters the path rules refuse (controls, noncharacters, ZWJ and ZWNJ, bidirectional controls, other invisible format characters); a non-ASCII name itself is accepted at repository format 8
- duplicate paths and case-insensitive collisions

Use `prikk checkout --snapshot-materialize` to write validated snapshot files. Supported patch replay
is available separately, while full patch algebra remains deferred. For exact validator rules, see
[path and worktree safety](../../reference/path-safety.md).
