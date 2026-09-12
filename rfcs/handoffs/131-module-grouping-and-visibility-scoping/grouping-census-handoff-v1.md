# RFC 131 §6e — a grouping census of `prikk-store`'s 52 top-level modules

**Ruled:** `rfcs/accepted/131-module-grouping-and-visibility-scoping.md` §6e. **Live.** This round
**groups nothing**; it produces the census the first grouping increment is chosen from.

**Why a census and not a grouping.** §6d.7 aimed a narrowing round at RFC 130's middle-hubs on §3's
advice and got 0 of 15 — value and availability ran opposite. Grouping has the same trap in a different
form: §6a/§6c showed a grouping can *hide* a declared cycle, and §2 forbids inventing a home for a
module that has none. **Measure first.**

## 1. The census, one row per top-level module (52 today)

| module | files / lines | role family (§2) | name family (§2) | fan-in / fan-out (gate) | in a declared cycle? | proposed home, or "singleton — stays" |

- **Role family** — foundation, orchestrator, or one you find and name; §2's two suggestions are
  starting material, not the answer.
- **Fan-in / fan-out** from `boundary-check`'s own graph, not from reading imports. Use the gate's
  numbers.
- **Declared cycle** — from `DECLARED_CYCLES` in `tools/release-policy/src/boundary/coupling.rs`.
- **"Singleton — stays"** is a valid, expected answer. §2 counted 38 of 69; say how many remain.

## 2. For each proposed grouping, the two checks §6a/§6c made mandatory

1. **Would the grouping merge two modules that are in a declared cycle with each other?** If so, the
   gate must still report the cycle after the move (§6c's subtree-aware detection exists for this) —
   **run it on a scratch branch and quote the result**, do not reason about it.
2. **Would it expose a new cycle** between the group and something outside it? Same: run, quote.

## 3. Report

- The full table.
- **A ranked list of at most three candidate first increments**, each with: modules moved, file count,
  reference rewrites estimated (grep `crate::<mod>::` callers), allowlist entries that rename, and the
  two §2 checks' results.
- **What you would not group and why** — the singleton count and the reasoning, because that is half
  the census.
- **Nothing moves in this round.** If you find a one-file, zero-caller move that is obviously safe,
  list it first; do not do it.

## 4. Controls

- The census's fan-in/fan-out must be reproducible from `boundary-check` output — say the command.
- Any cycle claim in §2 must be a quoted gate run, not an inference.
- Full gate set is not required for a census that changes no code; **say so explicitly** rather than
  omit it, and run `boundary-check` once to confirm the tree you measured is green.
