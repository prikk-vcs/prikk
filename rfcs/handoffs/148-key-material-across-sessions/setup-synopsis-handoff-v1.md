# RFC 148 — `setup`'s help synopsis still says "print the exports"

**Scheduled:** 0.41.0, small. **Live.** Found at the 0.40.0 readiness check (cut procedure step 1: every
shipped flag and synopsis against its own `--help`).

`prikk --help`, `commands.rs`:

```
prikk setup [repo-path] [--author-seed-out <path>] [--maintainer-seed-out <path>]  Init, generate both keys, trust the maintainer key, and print the exports
```

Since RFC 148 the default path prints nothing to export and, on a second project, generates nothing —
it reuses. The sentence is true only for user-named `--*-seed-out` paths. Rewrite the description to
what the command does now (init; keys in your key directory, created or reused; maintainer key
trusted), keep the flags, and mirror it in `commands.md`. The RFC 146 §8e control reads synopses for
flags, not descriptions, so nothing asserts this — say in the report whether a description-currency
check is worth adding or whether this stays a cut-time reading.

Full gate set; no `cfg` in a one-line string change, state it from the diff.
