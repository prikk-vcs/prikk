# RFC 132 per-site — `snapshot-plan`'s by-design absence is a precondition; and one stale docs sentence

**Scheduled:** 0.40.0 plan item 5, **authorized 2026-09-12**. Two one-line items in RFC 132's per-site
mould. **Live.**

## 1. `checkout --snapshot-plan --ref <ref>` on a block with no snapshot

Today: `error: integrity error: checkout target for tags/v1 does not contain a snapshot blob`. A block
with no snapshot is **by design** — RFC 136 §7 ruled snapshots are not written by any block-creating path
yet — so this is a precondition, not damage. Reclassify to `Precondition`; keep the fact; add the step
(`--patch-plan` materializes without a snapshot). Perturb: flip back, one test fails, restore.

## 2. `docs/src/guide/security-setup.md:22`

*"Prikk does not currently enforce a repository-wide AUTHOR trust policy."* DC-53 delivered
repository-wide author trust verification. **Verify what `verify` does today against a patch from an
unadopted author key — run it, quote it — then correct the sentence to what is true**, or report that it
is still true and why. Do not edit from memory of DC-53.

## 3. Controls

Full gate set, verbatim; cross-target outcome stated from the diff. `troubleshooting.md` gets the new
message with the old wording named, per the established pattern.
