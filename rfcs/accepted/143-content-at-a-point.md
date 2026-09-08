# RFC 143 — Content at a point: exposing the replay result we already compute

**Status.** **ACCEPTED by the project owner 2026-09-08**, the same day it was opened, at the stikk
project's request (`005-content-at-a-point.md`).

**What the acceptance covers, stated because a bare acceptance is scope-ambiguous:** §5's shipped
surface (a read-only `--format json` on `checkout --patch-plan`, **path-scoped**, binary as id and size
only), §6's requirement that the replay's partial coverage be machine-readable, and §7's refusals —
`prikk diff` stays refused, nothing writes, no whole-tree default.

**What it does not settle, and the implementing increment decides:** the schema name, the exact field
shape, and whether `--patch-materialize`'s report gains the same format. §7 lists these as open and
they remain so.

**Author's note on independence:** this RFC is written by the architect who refused `prikk diff` in
RFC 142 §5/§7. The refusal is not being softened here and §7 restates it. Reviewed against that risk
rather than assumed free of it.

---

## 1. Why this is opened now

RFC 142 §5 shipped `prikk show` and refused `prikk diff`, and the reply to stikk asked directly:
*"If your two designed views need arbitrary-point comparison rather than per-block content, tell us."*

**They answered: one does, and they are not asking for `diff`.** Their `FR-033` (*compare two blocks*)
folds from shipped surfaces except in one case — **a path changed more than once in the range.** A file
edited, reverted, and edited again yields three spans and no answer to *what is different between these
two points*. They correctly refuse to close it by reimplementing our replay.

**They ask instead for the replay result we already compute, in machine-readable form.**

## 2. The finding: the bytes exist and we discard them

**Verified at source, not accepted from the letter.**
`crates/prikk-store/src/patch_replay.rs`:

```rust
pub(crate) struct ReplayManifestEntry {
    pub(crate) path: RepoPath,
    pub(crate) bytes: Vec<u8>,   // the content
    pub(crate) mode: u32,
}

fn total_content_bytes(&self) -> u64 {
    self.files.iter().map(|entry| entry.bytes.len() as u64).sum()
}
```

**`prepare_patch_replay_plan` materializes every file's bytes into memory, then reports the sum of
their lengths and the path list, and drops the content.** The letter's claim — *"you already produce
the state at a point and print its size instead of its bytes"* — is exactly what the code does.

**This is why the ask is not the thing RFC 142 refused.** §7 there objected to *commissioning new
computation* and to owning comparison semantics. **This commissions no computation.**

## 3. Their supporting argument that does not hold, corrected here

The letter argues that `checkout` is *"the one read command with no `--format json`"* and that adding
one follows our own pattern. **Measured: it is not.** Nine read commands lack it — `log`,
`worktree-status`, `merge-evidence`, `merge-plan`, `inverse-plan`, `rollback-preview`, `doctor`,
`branch`, `checkout` — against five that have it (`show`, `status`, `verify`, `trust maintainer list`,
`trust maintainer check`).

**`--format json` is the exception on our read surfaces, not the rule.** The argument from consistency
is unavailable to this RFC and is not used by it. **The case rests on §2 alone, which is sufficient.**

## 4. What the replay actually covers, and why that is safe to expose

`ensure_apply_supported` (`patch_replay/decode.rs:128`) admits `CreateFile`, `DeleteNode(file)`,
`EditText`, `ReplaceBinary`, `ChangePerm`, and **returns an error** for `RenamePath`, `CreateSymlink`
and `DeleteNode(symlink)`. `single_parent_chain` walks one parent, so **merge blocks are outside it.**

**The refusals are errors, not silent omissions — the replay cannot emit partial content as though it
were complete.** `RenamePath` and `CreateSymlink` are also unauthored today (`commit` turns renames
into delete+create; symlink authoring is refused), so ordinary repositories replay cleanly.

**That is the property that makes this exposable at all**, and §6 makes it explicit rather than
implicit.

## 5. RULED — what this would ship

**A `--format json` on the existing read `checkout --patch-plan`. Not a new command.**
`prepare_patch_replay_plan` is already read-only — verified at `patch_replay.rs:158-161`, whose own
comment records that every production caller only reads and asks a future writing caller to re-check
that assumption.

**Content is emitted per requested path, never for the whole tree by default.** **A document carrying
every file's content at a point is unbounded in size and is not what the requesting view needs** —
`FR-033` needs content for the paths that differ, which it already knows.

**CORRECTED 2026-09-08 — this section originally said `checkout --patch-plan` "already accepts
`[path]`" and that the cost bound therefore already existed. That was false.** `checkout`'s positional
resolves to `CheckoutArgs.root: PathBuf` — **the repository root**, the same convention every other
command uses (`init`, `log`, `merge-evidence`, `rollback-preview`). **There is no content-path scoping
anywhere in this CLI**, so the mechanism this RFC leaned on did not exist. Found by the implementing
round, verified at `crates/prikk-cli/src/args/checkout.rs:13-14`.

**The intent survives; the mechanism is new.** A `--content-path <repo-relative-path>` flag, repeatable,
validated at parse time so a malformed path is a usage error (exit `2`) rather than a later integrity
failure. **The bound is real, it just had to be built rather than borrowed.**

**Binary content is never rendered.** `ReplaceBinary`-derived and binary-kind content report id and
declared size only, exactly as RFC 142 §7 rules for `show`. **One project, one answer to that
question.**

## 6. RULED — the replay's boundary must be machine-readable

**RFC 140 §7b and RFC 142 §6b apply, and this is their third instance:** a read may degrade, but it
must say so in a field a machine can branch on — and **absence degrades while an error propagates.**

**A consumer must be able to tell "this is the complete state at this point" from "this is what the
supported subset reconstructs" without parsing prose.** The document states the replay's own coverage:
which operation kinds were applied, and that the walk was single-parent. **A view built on this will
inherit our gaps, and the stikk letter says plainly they intend to name them in their UI — they can
only do that if we hand them the fact.**

## 7. What this does NOT decide, and what it refuses

**`prikk diff` is still refused, and this RFC does not reopen it.** RFC 142 §5's split stands: we
answer *what did this block change* and *what is the content here*. **We do not answer *what is
different between two arbitrary points*, and we own no comparison semantics.** The requesting project
takes the comparison and the responsibility for it; that is their proposal, not our concession.

**Refused here:** any writing behaviour on this path; any snapshot production (RFC 136's decision, not
this one); rendering binary bytes; and emitting whole-tree content by default (§5).

**Not decided here:** the schema name, the exact field shape, and whether `--patch-materialize`'s own
report gains the same format. Those belong to the implementing increment.

## 8. Scope

**In:** a read-only `--format json` for `checkout --patch-plan`, path-scoped, carrying content and mode
per path plus a machine-readable statement of replay coverage.

**Out:** comparison of any kind; new computation; new stored state; binary rendering; changes to
`prikk-object`; any of the eight other read commands lacking JSON (§3) — **this RFC is not a licence to
sweep them.**

## 9. Risks

- **A new external contract over a partial replay.** Bounded by §6, and by the fact that unsupported
  operations error rather than degrade — but a consumer that ignores the coverage field will still
  mistake partial for complete. **The field cannot force them to read it.**
- **Cost is unmeasured.** RFC 133 §5d recorded `show`'s replay cost on a depth-only fixture; this path
  shares `replay_supported_patch_chain`. **The implementing increment measures it and files it into
  RFC 133**, as RFC 139 and RFC 142 both did.
- **It makes a partial replay more visible, which raises pressure to complete it.** Renames in
  particular. That is a real consequence of shipping this and is stated rather than discovered later.
