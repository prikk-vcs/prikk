# RFC 146 §8e — the three listing synopses must advertise the flag they accept

**Ruled:** `rfcs/accepted/146-machine-readable-listings.md` §8e. **Live.** Shipped in 0.39.0; fix in the
next release.

**What is wrong, verified on the crates.io 0.39.0 binary.** `prikk log|branch|tag --format json` all
answer their schemas — but `prikk --help` says nothing about it:

```
commands.rs:173   "  prikk branch [list] [--all]                List branches …"
commands.rs:183   "  prikk tag [list]                          List tags …"
commands.rs:201   "  prikk log [path] [--limit N] [--ref REF]  Show sealed ref history …"
```

Nine other synopses carry `[--format json]`. These three now accept it and do not say so.

**Why it happened — the architect's rule, not your omission.** The RFC 146 handoff said *"no prose
change; the human output is not touched."* The help synopsis is prose, and the rule did not exempt it.
Recorded in §8e as mine.

## 1. The change

Add `[--format json]` to the three synopsis lines, matching the placement the other nine use. **Grep for
a second inventory** — `main.rs` once carried its own stale command list (`ed5c927` fixed it); if any
other file renders these lines, it moves in the same round.

## 2. Controls

1. **A test that derives the check rather than listing it**: for every command whose argument parser
   accepts `--format json`, its `--help` synopsis line contains `[--format json]` — **one assertion
   over the parser's own table**, so the next command to gain the flag cannot ship the same gap.
2. **Perturb**: remove the flag from one synopsis, the test names that command, restore.
3. **The three commands still behave** — schemas unchanged; this round touches help text only.
4. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; cross-target: state the outcome from this
   round's own diff.
5. **Docs**: `docs/src/reference/commands.md` — if it mirrors the synopsis, it moves too.

## 3. Not in this round

- No schema change, no behaviour change, no fourth command.
