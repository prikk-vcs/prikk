//! Merge: deciding whether two histories can be joined, and joining them.
//!
//! `evidence` reads two blocks against a baseline and reports what a merge would face — conflicts,
//! witnesses, and the per-operation detail `merge-evidence` and `merge-plan` render; it writes
//! nothing. `execute` takes the same three blocks and, only when the evidence proves confluence,
//! seals the merged block.
//!
//! RFC 131 §6f, grouping increment 2. One name family and one role family: the read half and the
//! write half of a single verb, with the read half a precondition of the write half.
//!
//! **`evidence` is a declared coupling hub** (`DECLARED_HUBS`, 7 in / 10 out). Grouping does not
//! hide it — the gate re-detects it at `merge::evidence` and reported the now-stale `merge_evidence`
//! entry in the same run, which is the allowlist rename this increment cost.

pub(crate) mod evidence;
pub(crate) mod execute;
