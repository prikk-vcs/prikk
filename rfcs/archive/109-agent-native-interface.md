# RFC 109 — Agent-native interface

**Status.** **ARCHIVED 2026-09-13 on the owner's authorization** — a concept record with no requirement behind it: no consumer has asked for an agent-native interface; the machine-readable surface grows by consumer ask (RFCs 140–150). Returns the day a requirement names it. Earlier: **Proposed** — concept under discussion. Authored by the project owner 2026-08-18; recorded
here by the architect with analysis. **No design exists and implementation must not start from this
record.**

**Tracks.** Product positioning and a new interface surface, not a correction.

## 1. The vision

prikk should be **not only intuitive for human developers but natively optimized for AI agent
interaction** — a unified dual-interface tool serving human intuition and machine intelligence with equal
priority.

Git's CLI was designed for human eyes and manual commands. In an era of autonomous and collaborative
agents that produces friction in four places: **token efficiency, trial-and-error safety, semantic
understanding, and history metadata.**

**Value propositions:**

- **Dual interface** — an ergonomic CLI for humans alongside structured JSON output and an embedded `mcp`
  subcommand for agents, in one lightweight binary.
- **Semantic diffs** — structural metadata instead of token-heavy line diffs, so an agent grasps "an
  argument was added to `calculate()`" without spending its context window on text.

## 2. The five shortcomings this addresses

1. **No structured data, poor token efficiency.** `git status`/`git diff` emit prose; agents parse strings
   and burn context on line-by-line text when they need "which file and which function changed, and how."
2. **Text diffs are not semantic diffs.** Re-indentation or moving a function reads as mass deletion plus
   mass addition, inducing misinterpretation.
3. **No transactionality or sandboxing for trial and error.** `git reset --hard` and `git checkout .` lose
   in-progress work; running several hypotheses in parallel needs heavyweight `worktree` or branch
   switching.
4. **Generation lineage is lost.** Commits record author and message. Which model, which prompt, which
   context files — unrecorded, with `git notes` as a non-standard workaround.
5. **No guardrails for destructive commands.** A human running `reset` intends "revert safely" and backs
   up first; an agent executes literally. Agent-oriented interfaces need shadow commits, dry-run previews,
   and guaranteed undo.

---

# Architect's analysis

## 3. prikk already holds more of this than the document assumes

Verified against the code, not inferred:

- **The patch model is operation-based, not line-based.** `payload/patch.rs` defines `CreateFile`,
  `DeleteNode`, `EditText`, `RenamePath`, `ChangePerm`, `CreateSymlink`, `ReplaceBinary`, with
  preconditions `OldContentHash`, `AnchorExists`, `PathExists`, `PathAbsent`. **`RenamePath` is a
  first-class operation** — prikk records "this moved" where git reconstructs it from similarity, which is
  §2.2's complaint answered by the existing data model.
- **`EditText` uses text anchors**, not line numbers — so a shifted region is not a mass rewrite.
- **The destructive operations §5 warns about do not exist.** There is no `reset --hard`, no amend, no
  rewrite, no force-push. History is append-only and immutable **by construction**.
- **Worktree writes already refuse rather than clobber.** `materialize_entry` returns an Integrity error
  rather than overwriting a file whose content differs.
- **The dry-run surface is already broad**: `checkout --plan-only`/`--snapshot-plan`/`--patch-plan`/
  `--patch-delete-plan`, `merge-plan`, `merge-evidence`, `inverse-plan`, `rollback-preview`,
  `rollback-draft-verify`.
- **Authorship is signed per change**, which is part of §4's provenance already.

**So §5's "safer interface" wish list is largely prikk's existing architecture rather than a feature to
add.** The gap is **exposure, not representation.**

**And there is no JSON output anywhere in the CLI.** Searched; none. So §1's dual-interface half is
entirely unbuilt, and it is the half where the work actually is.

## 4. Three tiers, ordered by the obligation each creates

The concept treats "structured output" and "AST diffs" as one idea. They carry very different costs and
should be separated before anything is designed.

| Tier | What it is | Obligation created |
|---|---|---|
| **1 — Expose** | Emit as JSON what prikk already stores: operations, renames, preconditions, anchors, signatures, plan output | **None beyond output stability.** Pure interface work |
| **2 — Derive** | Compute richer views on demand from sealed bytes — "which functions changed" — without storing them | **None permanent.** A derived view may change as tooling improves, because nothing depends on reproducing it |
| **3 — Store** | Seal AST metadata or generation provenance into blocks | **Permanent, and irreversible.** See §5 and §6 |

**Tier 1 alone answers most of §1 and §2** at a fraction of the cost, and it is the increment I would take
first regardless of what follows.

## 5. The AST trap: sealed AST metadata is a compatibility promise prikk cannot keep

AST-level diffs require parsing source languages — one grammar per language, in practice `tree-sitter` or
equivalent.

**If AST metadata is sealed into history, prikk owes grammar stability forever.** A verification claim made
today — "this block records that `calculate()` gained an argument" — is only reproducible if the same
grammar, same version, parses the same bytes the same way in ten years. Grammars change; languages change.

**This collides directly with badge criterion 2**, whose own formulation is *"what minimum must never
change for a verification claim made today to hold in ten years."* Sealing parsed structure adds every
supported language's grammar to that minimum.

**Ruling I would propose now, before any design: AST-derived views are Tier 2, never Tier 3.** Compute them
on demand from the sealed bytes, which are the thing prikk actually guarantees. Then grammar drift changes
what a *view* says, never what history *means* — and prikk supports a new language by adding a parser, not
by migrating a format.

## 6. The provenance trap: no rewrite means prompts are permanent

§4 wants model, prompt and context recorded. The auditability argument is strong and fits prikk's
positioning.

**But prikk has no rewrite.** Anything sealed is permanent, and prompts routinely contain proprietary
context, customer data, credentials pasted by accident, and internal URLs. **A leaked secret in a sealed
prompt cannot be removed** — not by amend, not by rebase, not by force-push, because none exists.

**And model identifiers decay.** "claude-opus-5" is not self-describing in ten years without a registry
prikk would then owe.

**So the design question is not "record provenance" but "what is the minimum durable provenance worth
permanence?"** A content hash of the prompt plus a model identifier is verifiable, non-leaking, and
compact; the prompt body itself may belong outside sealed history, or nowhere. **That question must be
answered before any provenance field is sealed**, because the mistake is unfixable by construction.

## 7. This concept and RFC 108 are the same concept from opposite ends

§3's *"run multiple modification patterns in parallel and keep only the successful ones"* is precisely what
RFC 108's Workspaces provide. **One document describes the mechanism, the other the motivating use case.**

That is worth stating because it changes how both should be scoped: **the agent use case is the strongest
argument for Workspaces**, and Workspaces are the answer to this document's §3. If they are designed
independently they will each invent half of the other.

## 8. The cost nobody has priced: agents multiply history

An agent-native tool will be used by agents, which commit far more often than humans.

**`verify` is roughly O(N³)** — 34 seconds at 160 blocks, recorded as badge criterion 3, measured and
unowned. An agent-native prikk makes that **existential rather than pressing**: the tool's own positioning
would guarantee the growth rate that breaks its central claim of offline verifiability by anyone.

**Criterion 3 is a prerequisite of this vision, not a parallel concern.**

## 9. MCP is a transport, not a capability

An `mcp` subcommand is a reasonable delivery mechanism, but the capability is **structured output plus safe
operations**. MCP is one way to carry it, and protocols change.

Design the capability first and let MCP be one adapter over it. Otherwise prikk's agent interface inherits
a protocol's lifecycle, and the next protocol requires re-doing the work rather than adding an adapter.

## 10. What a design must answer first

1. **Tier 1's output contract.** Which commands, what schema, and — the part that will be skipped if not
   named — **is the JSON schema a compatibility surface?** If agents parse it, changing it breaks them, and
   prikk has no versioning story for it yet.
2. **§5's ruling**, confirmed or overturned: AST as derived view, never sealed.
3. **§6's minimum durable provenance**, decided before anything provenance-shaped is sealed.
4. **Whether this and RFC 108 are one programme or two**, per §7.
5. **Criterion 3's standing**, per §8 — whether it becomes a prerequisite.

## 11. Non-goals to state explicitly

- **Not a git-compatible interface.** The premise is that git's shape is the problem.
- **Not agent autonomy features** — approval flows, policy engines, sandboxed execution. This is about the
  interface prikk presents, not what an agent is permitted to do with it.
- **Not sealing parsed structure**, per §5, unless that ruling is explicitly overturned.
