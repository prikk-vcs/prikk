# A queued patch's operations can name their paths — handoff v1

**Live 2026-09-22.** From **stikk's letter 016**, after the architect measured it: the defect they reported does
not exist (`show <block-id> --format json` already carries the path on every operation, on the released 0.46.0
binary), but the case underneath it is real and is this round.

**Scheduled in 0.47.0 after RFC 158 Stage A, ahead of the gate plan.**

## 1. What is true today, measured on 0.46.0

One patch with all six operation kinds:

| invocation | `edit-text`, `replace-binary`, `change-perm` |
|---|---|
| `show <block-id>`, prose **and** `--format json` | the path, resolved |
| `show <patch-id>`, prose **and** `--format json` | `unresolved node <id>` / `"unresolved_node_id"` |

Both formats agree in both cases — `show()` passes `Some(lifecycle_state_at(block))` for a block and `None` for a
bare patch id, and one `ShowPatch` renders into either format. **A queued patch takes the `None` path**
(`show_queued_patch`), which RFC 142 §7a states deliberately: *"no block context, so node-addressed operations
report unresolved"*.

## 2. What to build

**`show` resolves a queued patch's node paths from the folded baseline up to and including that patch.**

- the state is the one `commit` and `worktree-status` already derive — the sealed tip with the queue folded on
  top — **truncated at the patch being shown**, so the answer is the state *at* that patch, exactly as
  `lifecycle_state_at(block)` is the state at that block. A patch must render identically before and after
  `seal` apart from `"queued": true`;
- **`unresolved_node_id` stays** for a node with no live path in that state, and the field is not removed from
  the schema;
- **a bare sealed patch id keeps answering unresolved.** RFC 142 §1 is explicit that a patch id alone carries no
  block context, and searching for a block that contains it would make one command's answer depend on what else
  happens to be sealed. Do not.

**Read-only, as `show` is today**: derive the baseline without refreshing the cache — the switch RFC 153 added
(`CacheWrite::Never`) exists for exactly this, and `show` must not become a writer.

## 3. Controls

1. **Identical before and after `seal`.** One patch with all six kinds: capture `show <patch-id> --format json`
   while queued, `seal`, then capture `show <patch-id-in-its-block>`'s entry for the same patch from
   `show <block-id> --format json`. **The two operation lists are equal**, paths included. *Perturb: resolve
   against the tip instead of the folded baseline — the control must go red.*
2. **The truncation matters**, so it gets its own control: **two** queued patches where the second renames a
   file the first edited. The first patch's `edit-text` must name the path **as it was at that patch**, not the
   later one. *Perturb: fold the whole queue for every patch — red.*
3. **A node with no live path still reports `unresolved_node_id`** — a queued patch whose node the queue later
   deletes, or any state where the lookup genuinely fails.
4. **A bare sealed patch id is unchanged**: still `unresolved_node_id`, and its existing test still passes
   untouched.
5. **`show` writes nothing**: `.prikk/` byte-identical across `show <queued-patch-id>` run right after a `seal`,
   the control shape RFC 153 §7.1 already uses for `diff`.

## 4. Docs and CHANGELOG

- `guide/show.md`: say which forms resolve a path and which do not, in one table — the block form, the queued
  form (new), the bare sealed patch id (not resolved, and why);
- **CHANGELOG `### Changed`** (not `Fixed` — nothing was broken): a queued patch's node-addressed operations now
  carry `path` in prose and in `show-report-v1`, where they carried only `unresolved_node_id`. A consumer that
  keyed on the field's absence needs to know.

## 5. Not in this round

The bare sealed patch id (§2), and anything about `--format json`'s shape beyond the one added field.

**Report:** `.git-exclude/review-request/queued-patch-paths-report-v1.md`.
