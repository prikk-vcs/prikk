# RFC 144 — the bundle-impact preview

**Design:** `rfcs/accepted/144-two-point-comparison.md` **§4m** (§4m.1–§4m.5), building on §4f's shape
ruling and §4g.3's separability ruling. **Independent of the rename work** — §4g.3 established it needs
no revisiting when the rename question settles, so it does not wait on increment 3.

## 1. What you are building

**`prikk bundle preview --input <file>`** — a `bundle` subcommand beside `export` / `import` / `verify`,
answering *what would this bundle do to my repository* **without admitting its objects into the store.**

**The gap, so the shape is not guessed** (§4m.1): `bundle verify` answers *is this bundle sound* and needs
no repository. `merge-plan` is already read-only but takes block ids or refs, so it can only answer
**after** `bundle import` has written the bundle's objects. **Today there is no way to ask the question
before letting foreign objects into your object store.** This is that step.

## 2. REQUIRED — it writes nothing, and you must prove it

No objects, no refs, no received pointer, no trust state, no cache write. **A safety surface that mutates
the thing it protects is not one.**

**Prove it with a control, not by inspection.** Capture the repository's full on-disk state before and
after a preview run — every object, every ref, every cache file — and assert byte-identity. **Then
perturb**: make the implementation write one object and confirm the control fails. A control that cannot
fail is this project's most frequently repeated defect; see RFC 142's three rounds.

Note the cache in particular: replay paths persist an incremental lifecycle cache best-effort. **Preview
must not take that path**, or it will write while reporting that it does not.

## 3. The reader — the implementation crux (§4m.3)

The bundle is self-contained, so its objects must be **readable without being written**. Build a composed,
read-only reader: the repository's object store **overlaid with the bundle's own object set held in
memory**, and run replay against that.

Do not solve this by importing to a temporary directory and deleting afterward. That writes, it can leave
residue on a crash, and it makes the write-nothing control a statement about cleanup rather than about
behaviour.

## 4. What it reports

**Effects at node granularity** — paths created, deleted, edited, renamed, permission-changed, with counts
and paths. This is exactly what the model can honestly say today.

**Three answers that are not effects (§4m.3), and are the point of the surface:**

1. **Does it apply at all?** A bundle whose history does not connect to this repository's is a legitimate,
   common answer. **Report it; do not error.**
2. **Would it conflict?** A conflict is the result of a successful preview, not a failure of one.
3. **Who sealed it?** Report the sealing identity **and state in the same breath that this is not a trust
   decision.** `bundle import` already prints exactly that disclaimer about recorded author key material —
   match its care. A preview that implies trust it has not established is worse than one that says nothing.

## 5. Exit codes and output (§4m.4)

**RFC 121 unchanged: `0` preview produced · `1` operational failure · `2` usage error.**

**A bundle that would conflict, or that does not connect, exits `0`.** The command was asked what would
happen and it answered.

**So the answer must be machine-branchable in the output** — *applies cleanly*, *applies with conflicts*,
*does not connect* must be distinguishable from a **field**, never by parsing prose and never by inferring
from an exit code. That is RFC 140 §7b / RFC 142 §6b's lineage and it is not negotiable. **`--format json`
is required**, not optional.

**Degradation: degrade absence, propagate error** (RFC 142 §6b). An unreadable blob is reported unavailable
in a named field; a malformed bundle is an error.

**State the honesty limit in the output itself**, not only in the docs: until rename authoring lands, a
bundle containing delete+create for a move previews delete+create (§4g.3). **The output must say so**
rather than let a reader conclude that was the author's intent.

## 6. Out of scope — do not add these

- **Content-level diffing of any kind.** §4f declined it deliberately; a caller composes it from RFC 143.
- Any trust evaluation, and any change to `import`, `merge`, or `merge-plan`.
- **Any offer to import.** The preview tells you; it does not act, and it does not prompt.
- No `CHANGELOG.md` guess — this is user-visible, so it **does** need a changelog entry; write one.

## 7. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9, against your final commit. New CLI surface means
`rfc121_command_help` and the argument-hygiene tests apply — check `main.rs`'s own command inventory is
updated, which a previous round found stale.

Report to `.git-exclude/review-request/`. Lead with the write-nothing control and its perturbation result;
that is the claim the whole surface rests on. State plainly which of §4m.3's three answers you could
produce and which, if any, the current model cannot support — **an honest "this one is not answerable
today" is a better result than a field that is always populated and sometimes wrong.**
