# Command Surface

Every command prikk accepts, in one place. This page is an inventory, not a tutorial — each command's
own behaviour, refusals, and worked examples live in the [guide](../guide/tutorial.md), and the
[Git→prikk mapping](git-mapping.md) explains where the vocabulary differs from Git's.

Every command accepts `--help` for its own usage:

```text
prikk <command> --help
```

```text
prikk init [path]
prikk key generate [--out <path>]
prikk key public --seed-env <NAME>
prikk setup [repo-path] [--author-seed-out <path>] [--maintainer-seed-out <path>]
prikk trust maintainer add --key-id ID --public-key HEX
prikk trust maintainer remove --key-id ID
prikk trust maintainer list [--format json]
prikk trust maintainer check --key-id ID [--format json]
prikk commit [--from-worktree] [--text-edits] [--ref heads/<branch>] -m <message>
prikk mv <old> <new>
prikk seal --allow-no-audit [--ref heads/<branch>]
prikk status [--format json]
prikk log [path] [--limit N] [--ref REF]
prikk checkout --plan-only [path] [--ref REF]
prikk checkout --snapshot-plan [path] [--ref REF]
prikk checkout --snapshot-materialize [path] [--ref REF]
prikk checkout --patch-plan [path] [--ref REF] [--format json [--content-path <path>]...]
prikk checkout --patch-materialize [path] [--ref REF]
prikk checkout --patch-delete-plan [path] [--ref REF]
prikk checkout --patch-materialize-delete [path] [--ref REF]
prikk show <block-id|patch-id> [--format json]
prikk merge-evidence --baseline-block ID (--left-block ID|--left-ref REF) (--right-block ID|--right-ref REF) [path]
prikk merge-plan --baseline-block ID (--left-block ID|--left-ref REF) (--right-block ID|--right-ref REF) [path]
prikk merge --allow-no-audit --baseline-block ID --into REF --from REF [path]
prikk inverse-plan [path] [--ref REF]
prikk rollback-preview [path] [--ref REF]
prikk rollback-draft --append-inverse [path] [--ref REF] -m <message>
prikk rollback-draft-verify [path] [--ref REF]

prikk branch [list] [--all]
prikk branch create heads/<name> [--from REF]
prikk branch close heads/<name>
prikk tag [list]
prikk tag create tags/<name> --target <ref|block> [-m <message>]
prikk bundle export --ref REF --output <file> [--force]
prikk bundle import --input <file>
prikk bundle verify --input <file>
prikk sync summary --output <file>
prikk sync compare --summary <file>
prikk sync have <ref> --output <file>
prikk sync build <ref> --have <file> --output <file> [--force]
prikk sync accept <file> [--claims-out <file>] [--force]
prikk sync pending
prikk sync seal <ref> --claim <id>
prikk sync seal <ref> --claims <file>
prikk sync tags
prikk sync adopt-tag <name>
prikk worktree-status [path] [--ref REF] [--format json]
prikk verify [path] [--stop-on-first-error] [--format json]
prikk doctor [path]
prikk doctor [path] --repair-wal-tail
prikk doctor [path] --repair-main-ref
prikk unlock
prikk unlock --lock <path> [--yes|--force]
prikk compact --pointer-index|--received-index|--trust-policy|--all [--plan-only]
```

**Exit codes.** `0` — the operation succeeded and did what was asked. `1` — operational failure:
verification findings, an integrity failure, a refusal, a dirty worktree. `2` — usage error: an
unknown argument, a missing required flag, a duplicate flag. Graded verification results are in
`prikk verify --format json`, not in the exit code.
