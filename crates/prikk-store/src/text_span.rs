//! Shared FDD-01 §5.1 text-span identity primitives (DC-09 Phase 4.4-2c-4, carry-forward C1).
//!
//! These compute **identity-bearing** bytes for content-anchored text edits: the bounded left/right
//! anchor hashes, the `span_id`, the overlapping occurrence enumeration, the anchor-filtered span
//! localization (Option A), the post-localization splice, and the derived `BlobPayload(Text, …)`
//! content `ObjectId`. They are the single source of truth for both authoritative lifecycle replay
//! and (later) worktree authoring — the two MUST NOT compute any of these through separate
//! implementations, or they would drift on identity bytes.
//!
//! The full identity chain is, in stages (kept separable for testing and conformance vectors):
//!   1. localization: current text + `EditText` record → `(start, end)` ([`locate_text_span`]);
//!   2. splice:       `(text, start, end, replacement)` → `new_text` ([`splice_text`]);
//!   3. identity:     `new_text` → [`text_blob_id`].
//!
//! Conformance is pinned by golden vectors in [`vectors`].
//!
//! **Sequencing invariant (RFC 134 §7.4 item 1, permanent — v2's schema 3 does not retire it):**
//! every `EditText` is authored against the state its predecessors produced — never against a
//! shared baseline. v1 identity depends on this because `dup_index` is recomputed against the
//! buffer at lookup; v2 depends on it because anchors are. **What upholds it**: `plan_edit_text`
//! (`worktree_patch/node_authoring.rs`) emits one operation per file per commit, and
//! `current_text_for_node` resolves through the queued-patch cache, then the stored blob, then
//! replay — so ordinary authoring can never produce two operations on the same node against the
//! same baseline. **What breaks it, and it is not hypothetical**: a sequence built
//! programmatically against one baseline (RFC 113's Git/Subversion/CVS import is the named case),
//! or a crafted or externally-produced patch. When it breaks, resolution correctly refuses — see
//! `patch_algebra::commutation::replay_sequence_order`'s own doc for what that refusal means.
//!
//! Split across three files (DC-58): this file keeps the identity primitives shared by both replay
//! and authoring (`locate_text_span`, `splice_text`, `text_blob_id`, `occurrences`,
//! `compute_span_id`, the anchor hashes); `authoring.rs` holds deterministic span selection for
//! worktree authoring; `inverse.rs` holds direct-inverse derivation. Both `pub(crate) use`
//! re-exported here so every existing `text_span::plan_authored_text_span` /
//! `text_span::derive_inverse_edit_text` caller is unaffected. No behaviour change.

use std::fmt;

use prikk_error::PrikkError;
use prikk_object::{BlobKind, BlobPayload, CanonicalEncode, NodeId, ObjectId, ObjectType};

mod authoring;
mod inverse;

// RFC 149 §6c: waypoint widened with its item; a `pub(crate)` re-export blocks `lib.rs` (E0364).
pub use authoring::plan_authored_text_span;
// Only `lib.rs`'s test-support block needs these two by this path; production code reaches
// `AuthoredTextSpan` through `authoring` directly.
#[cfg(test)]
pub(crate) use authoring::choose_anchor_lengths_v2;
#[cfg(any(test, feature = "test-support"))]
pub use authoring::{AuthoredTextSpan, TextSpanSelectionError};
// RFC 149 §6c: the v1 planner and its result type travel with `bundle`'s fixture.
#[cfg(any(test, feature = "test-support"))]
pub use authoring::{AuthoredTextSpanV1, plan_authored_text_span_v1};
// RFC 149 §6c: waypoint widened with its item; a `pub(crate)` re-export blocks `lib.rs` (E0364).
pub use inverse::derive_inverse_edit_text;

/// Canonical anchor context window (FDD-01 §5.1 anchor-window clarification): up to 64 bytes of raw
/// text on each side of the span, byte-exact, no normalization.
pub(crate) const TEXT_ANCHOR_WINDOW: usize = 64;

/// Why anchor-filtered span localization failed (FDD-01 §5.1). Shared with replay (which wraps it
/// with `node_id`/`span_id` context) and, later, worktree authoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TextSpanResolutionFailure {
    /// No occurrence's anchors matched the record's anchor hashes.
    AnchorMismatch,
    /// Anchors matched, but no anchor-filtered occurrence reproduced the record's `span_id`.
    NoMatchingSpanId,
    /// More than one occurrence reproduced the record's `span_id` (defensive; needs a collision).
    Ambiguous,
    /// RFC 134 §8: a record carried exactly one of `left_anchor_len`/`right_anchor_len`. Decode
    /// already refuses this; this is defense-in-depth for a caller that bypasses decode.
    MalformedAnchorLengths,
}

impl fmt::Display for TextSpanResolutionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::AnchorMismatch => "no occurrence's anchors matched the record",
            Self::NoMatchingSpanId => "no anchor-filtered occurrence reproduced the span_id",
            Self::Ambiguous => "more than one occurrence reproduced the span_id",
            Self::MalformedAnchorLengths => {
                "exactly one of left_anchor_len/right_anchor_len was present"
            }
        };
        f.write_str(s)
    }
}

/// Invalid byte range handed to [`splice_text`]. `locate_text_span` never produces one; the guard
/// exists for the later authoring caller (E1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// RFC 149 §6c closure: a type in the signature of an item the test-support surface exposes.
// Same rule as contract entries 42-46 -- a `pub` item returning or taking a private type is not
// reachable in any useful sense.
#[non_exhaustive]
pub struct TextSpanSpliceError {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text_len: usize,
}

impl fmt::Display for TextSpanSpliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid text-span splice range start={} end={} for text of length {}",
            self.start, self.end, self.text_len
        )
    }
}

impl std::error::Error for TextSpanSpliceError {}

/// `ObjectId` of the canonical `BlobPayload(Text, content)` — the content identity recorded for a
/// text node (FDD-03 §10.2 `node_payload`). Uses the existing blob encoding; no new identity bytes.
pub(crate) fn text_blob_id(content: &[u8]) -> Result<ObjectId, PrikkError> {
    let payload = BlobPayload::new(BlobKind::Text, content.to_vec());
    let bytes = payload.to_canonical_bytes()?;
    Ok(ObjectId::from_canonical_payload(
        ObjectType::Blob,
        1,
        &bytes,
    ))
}

/// SHA-256 of the bounded left context (up to 64 bytes preceding `start`), FDD-01 §5.1.
pub(crate) fn left_anchor(text: &[u8], start: usize) -> [u8; 32] {
    let lo = start.saturating_sub(TEXT_ANCHOR_WINDOW);
    anchor_hash(
        b"PRIKK-TEXT-LEFT-ANCHOR-v1",
        text.get(lo..start).unwrap_or(&[]),
    )
}

/// SHA-256 of the bounded right context (up to 64 bytes following `end`), FDD-01 §5.1.
pub(crate) fn right_anchor(text: &[u8], end: usize) -> [u8; 32] {
    let hi = end.saturating_add(TEXT_ANCHOR_WINDOW).min(text.len());
    anchor_hash(
        b"PRIKK-TEXT-RIGHT-ANCHOR-v1",
        text.get(end..hi).unwrap_or(&[]),
    )
}

fn anchor_hash(domain: &[u8], context: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(domain.len() + 4 + context.len());
    preimage.extend_from_slice(domain);
    preimage.extend_from_slice(&(context.len() as u32).to_be_bytes());
    preimage.extend_from_slice(context);
    prikk_hash::sha256(&preimage)
}

/// `span_id` per FDD-01 §5.1.
pub(crate) fn compute_span_id(
    node_id: NodeId,
    old_span_hash: &[u8; 32],
    left: &[u8; 32],
    right: &[u8; 32],
    dup_index: u32,
) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(18 + 32 * 4 + 4);
    preimage.extend_from_slice(b"PRIKK-TEXT-SPAN-v1");
    preimage.extend_from_slice(node_id.as_bytes());
    preimage.extend_from_slice(old_span_hash);
    preimage.extend_from_slice(left);
    preimage.extend_from_slice(right);
    preimage.extend_from_slice(&dup_index.to_be_bytes());
    prikk_hash::sha256(&preimage)
}

/// SHA-256 of the left context of exactly `len` bytes preceding `start` (RFC 134 §8), clamped at
/// the buffer start exactly as [`left_anchor`] clamps at its fixed 64-byte window. `len` is not
/// itself part of the domain-separated preimage; `anchor_hash` already folds in the clamped
/// context's own byte length, which is what makes every position's anchor distinct once `len`
/// reaches `start` (§8.3's uniqueness-always-achievable argument).
pub(crate) fn left_anchor_v2(text: &[u8], start: usize, len: u32) -> [u8; 32] {
    let lo = start.saturating_sub(len as usize);
    anchor_hash(
        b"PRIKK-TEXT-LEFT-ANCHOR-v2",
        text.get(lo..start).unwrap_or(&[]),
    )
}

/// SHA-256 of the right context of exactly `len` bytes following `end` (RFC 134 §8). See
/// [`left_anchor_v2`].
pub(crate) fn right_anchor_v2(text: &[u8], end: usize, len: u32) -> [u8; 32] {
    let hi = end.saturating_add(len as usize).min(text.len());
    anchor_hash(
        b"PRIKK-TEXT-RIGHT-ANCHOR-v2",
        text.get(end..hi).unwrap_or(&[]),
    )
}

/// `span_id` per RFC 134 §8: content-unique identity, no `dup_index`. The anchor lengths are
/// folded into the preimage as big-endian `u32`, matching [`compute_span_id`]'s existing
/// `dup_index.to_be_bytes()` convention -- chosen, not incidental, so this identity's byte
/// encoding follows the one precedent already established for a `u32` in this preimage shape.
pub(crate) fn compute_span_id_v2(
    node_id: NodeId,
    old_span_hash: &[u8; 32],
    left: &[u8; 32],
    right: &[u8; 32],
    left_len: u32,
    right_len: u32,
) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(18 + 32 * 4 + 8);
    preimage.extend_from_slice(b"PRIKK-TEXT-SPAN-v2");
    preimage.extend_from_slice(node_id.as_bytes());
    preimage.extend_from_slice(old_span_hash);
    preimage.extend_from_slice(left);
    preimage.extend_from_slice(right);
    preimage.extend_from_slice(&left_len.to_be_bytes());
    preimage.extend_from_slice(&right_len.to_be_bytes());
    prikk_hash::sha256(&preimage)
}

/// Canonical-order occurrences of `needle` in `text` (overlapping). For an empty needle, the
/// insertion positions `0..=len`.
pub(crate) fn occurrences(text: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() {
        return (0..=text.len()).collect();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= text.len() {
        if text.get(i..i + needle.len()) == Some(needle) {
            out.push(i);
        }
        i += 1;
    }
    out
}

/// Localize an `EditText` span in `text` (FDD-01 §5.1, Option A): among canonical-order
/// occurrences of `old_span_text` whose 64-byte anchor hashes match the record, find the one whose
/// recomputed `span_id` (using its zero-based index within that anchor-filtered list) equals the
/// record's. Requires exactly one. The stored `span_id` is recomputed, never trusted directly.
#[allow(clippy::too_many_arguments)]
pub(crate) fn locate_text_span(
    text: &[u8],
    old_span_text: &[u8],
    record_left: &[u8; 32],
    record_right: &[u8; 32],
    record_span_id: &[u8; 32],
    node_id: NodeId,
    old_span_hash: &[u8; 32],
) -> Result<(usize, usize), TextSpanResolutionFailure> {
    let span_len = old_span_text.len();
    let anchor_matching: Vec<(usize, usize)> = occurrences(text, old_span_text)
        .into_iter()
        .map(|start| (start, start + span_len))
        .filter(|&(start, end)| {
            left_anchor(text, start) == *record_left && right_anchor(text, end) == *record_right
        })
        .collect();

    if anchor_matching.is_empty() {
        return Err(TextSpanResolutionFailure::AnchorMismatch);
    }

    let mut matches = Vec::new();
    for (dup_index, &(start, end)) in anchor_matching.iter().enumerate() {
        let sid = compute_span_id(
            node_id,
            old_span_hash,
            record_left,
            record_right,
            dup_index as u32,
        );
        if sid == *record_span_id {
            matches.push((start, end));
        }
    }

    match matches.as_slice() {
        [] => Err(TextSpanResolutionFailure::NoMatchingSpanId),
        [one] => Ok(*one),
        _ => Err(TextSpanResolutionFailure::Ambiguous),
    }
}

/// Localize an `EditText` span in `text` (RFC 134 §8, v2): among canonical-order occurrences of
/// `old_span_text` whose anchor hashes at the *recorded* `left_len`/`right_len` match the record,
/// require exactly one, then recompute `span_id` and compare. **No `dup_index`** -- v2 disambiguates
/// occurrences by anchor length at authoring time, not by position at replay time, so this is a
/// tamper/consistency check, never a disambiguation step.
#[allow(clippy::too_many_arguments)]
pub(crate) fn locate_text_span_v2(
    text: &[u8],
    old_span_text: &[u8],
    record_left: &[u8; 32],
    record_right: &[u8; 32],
    record_span_id: &[u8; 32],
    node_id: NodeId,
    old_span_hash: &[u8; 32],
    left_len: u32,
    right_len: u32,
) -> Result<(usize, usize), TextSpanResolutionFailure> {
    let span_len = old_span_text.len();
    let anchor_matching: Vec<(usize, usize)> = occurrences(text, old_span_text)
        .into_iter()
        .map(|start| (start, start + span_len))
        .filter(|&(start, end)| {
            left_anchor_v2(text, start, left_len) == *record_left
                && right_anchor_v2(text, end, right_len) == *record_right
        })
        .collect();

    let (start, end) = match anchor_matching.as_slice() {
        [] => return Err(TextSpanResolutionFailure::AnchorMismatch),
        [one] => *one,
        _ => return Err(TextSpanResolutionFailure::Ambiguous),
    };

    let sid = compute_span_id_v2(
        node_id,
        old_span_hash,
        record_left,
        record_right,
        left_len,
        right_len,
    );
    if sid != *record_span_id {
        return Err(TextSpanResolutionFailure::NoMatchingSpanId);
    }
    Ok((start, end))
}

/// Localize an `EditText` span, dispatching between v1's positional identity and RFC 134 §8's v2
/// content-unique identity based on whether `left_anchor_len`/`right_anchor_len` are present.
/// **This is the single dispatch every production call site must share** (`patch_replay/apply.rs`
/// and the algebra oracle `patch_algebra/replay_oracle.rs` chief among them) -- if they diverge on
/// this decision, the algebra predicts something materialization does not do.
#[allow(clippy::too_many_arguments)]
pub fn resolve_text_span(
    text: &[u8],
    old_span_text: &[u8],
    record_left: &[u8; 32],
    record_right: &[u8; 32],
    record_span_id: &[u8; 32],
    node_id: NodeId,
    old_span_hash: &[u8; 32],
    left_anchor_len: Option<u32>,
    right_anchor_len: Option<u32>,
) -> Result<(usize, usize), TextSpanResolutionFailure> {
    match (left_anchor_len, right_anchor_len) {
        (None, None) => locate_text_span(
            text,
            old_span_text,
            record_left,
            record_right,
            record_span_id,
            node_id,
            old_span_hash,
        ),
        (Some(left_len), Some(right_len)) => locate_text_span_v2(
            text,
            old_span_text,
            record_left,
            record_right,
            record_span_id,
            node_id,
            old_span_hash,
            left_len,
            right_len,
        ),
        _ => Err(TextSpanResolutionFailure::MalformedAnchorLengths),
    }
}

/// Splice `replacement` into `text` over the located byte range `[start, end)`, returning
/// `new_text = text[..start] ‖ replacement ‖ text[end..]`. Rejects an invalid range (E1) rather
/// than clamping or panicking. The output bytes are the input to [`text_blob_id`], so this is the
/// single shared splice both replay and authoring must use.
pub fn splice_text(
    text: &[u8],
    start: usize,
    end: usize,
    replacement: &[u8],
) -> Result<Vec<u8>, TextSpanSpliceError> {
    if start > end || end > text.len() {
        return Err(TextSpanSpliceError {
            start,
            end,
            text_len: text.len(),
        });
    }
    let mut new_text = Vec::with_capacity(text.len() - (end - start) + replacement.len());
    new_text.extend_from_slice(text.get(..start).unwrap_or(&[]));
    new_text.extend_from_slice(replacement);
    new_text.extend_from_slice(text.get(end..).unwrap_or(&[]));
    Ok(new_text)
}

#[cfg(test)]
mod vectors;
