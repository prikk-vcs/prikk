//! Deterministic arbitrary-span text-edit authoring: selecting and identifying one span from an
//! old/new byte-content diff. Split out of `text_span.rs` (DC-58) — no behaviour change, all items
//! moved verbatim. Re-exported at `text_span.rs` so every existing `text_span::plan_authored_text_span`
//! caller is unaffected.

use std::fmt;

use prikk_object::NodeId;

use super::{
    TEXT_ANCHOR_WINDOW, compute_span_id_v2, left_anchor_v2, locate_text_span_v2, occurrences,
    right_anchor_v2,
};
use prikk_object::text_span_hash;

// RFC 149 §6c: the *import* widens with `plan_authored_text_span_v1`, which is its only user
// here. The four helpers themselves are untouched production `pub(crate)` items.
#[cfg(test)]
use super::splice_text;
#[cfg(any(test, feature = "test-support"))]
use super::{compute_span_id, left_anchor, locate_text_span, right_anchor};

/// A deterministic authoring plan for one span-anchored text edit. Always RFC 134 §8 v2 identity
/// -- new authoring never mints v1 (`dup_index`-positional) identity; v1 stays a replay-only,
/// frozen-forever concern.
#[derive(Debug, Clone, PartialEq, Eq)]
// RFC 149 §6c closure: a type in the signature of an item the test-support surface exposes.
// Same rule as contract entries 42-46 -- a `pub` item returning or taking a private type is not
// reachable in any useful sense.
#[non_exhaustive]
pub struct AuthoredTextSpan {
    /// Byte offset where the replaced span begins in the old text.
    pub old_start: usize,
    /// Byte offset where the replaced span ends in the old text.
    pub old_end: usize,
    /// Byte offset where the replacement begins in the new text.
    pub new_start: usize,
    /// Byte offset where the replacement ends in the new text.
    pub new_end: usize,
    /// The span's bytes before the edit.
    pub old_span_text: Vec<u8>,
    /// The bytes the span becomes.
    pub replacement_text: Vec<u8>,
    /// Hash of the span's text before the edit.
    pub old_span_hash: [u8; 32],
    /// Hash of the bounded context immediately left of the span.
    pub left_anchor_hash: [u8; 32],
    /// Hash of the bounded context immediately right of the span.
    pub right_anchor_hash: [u8; 32],
    /// Length of the left anchor this plan chose (RFC 134 §8 v2 identity).
    pub left_anchor_len: u32,
    /// Length of the right anchor this plan chose.
    pub right_anchor_len: u32,
    /// The v2 content-anchored span identity this plan mints.
    pub span_id: [u8; 32],
}

/// Why deterministic text-span authoring failed.
#[derive(Debug, Clone, PartialEq, Eq)]
// RFC 149 §6c closure: a type in the signature of an item the test-support surface exposes.
// Same rule as contract entries 42-46 -- a `pub` item returning or taking a private type is not
// reachable in any useful sense.
#[non_exhaustive]
pub enum TextSpanSelectionError {
    /// The old text is not valid UTF-8, so no text span can be selected in it.
    OldTextNotUtf8,
    /// The new text is not valid UTF-8.
    NewTextNotUtf8,
    /// The widened range does not lie on character boundaries of the old text.
    InvalidWidenedRange,
    /// RFC 134 §8.3 claims a unique anchor length always exists for a finite file. This variant
    /// exists only so a counterexample is reported, per the handoff's own instruction, rather than
    /// looping forever or silently accepting a non-unique span.
    AnchorUniquenessUnreachable,
}

impl fmt::Display for TextSpanSelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::OldTextNotUtf8 => "old text is not well-formed UTF-8",
            Self::NewTextNotUtf8 => "new text is not well-formed UTF-8",
            Self::InvalidWidenedRange => "widened text-span range is invalid",
            Self::AnchorUniquenessUnreachable => {
                "no anchor length up to the file's own size made this span unique"
            }
        };
        f.write_str(s)
    }
}

impl std::error::Error for TextSpanSelectionError {}

/// A selected span, before either identity scheme's tail is computed. The LCP/LCS selection and
/// UTF-8 boundary widening are identical between v1 and v2 -- only the anchor/identity computation
/// that follows differs.
struct SelectedSpan {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
    old_span_text: Vec<u8>,
    replacement_text: Vec<u8>,
}

fn select_span(old: &[u8], new: &[u8]) -> Result<Option<SelectedSpan>, TextSpanSelectionError> {
    if old == new {
        return Ok(None);
    }
    let old_text = core::str::from_utf8(old).map_err(|_| TextSpanSelectionError::OldTextNotUtf8)?;
    let new_text = core::str::from_utf8(new).map_err(|_| TextSpanSelectionError::NewTextNotUtf8)?;

    let prefix = common_prefix_len(old, new);
    let suffix = common_suffix_len(old, new, prefix);
    let mut old_start = prefix;
    let mut new_start = prefix;
    let mut old_end = old.len() - suffix;
    let mut new_end = new.len() - suffix;

    while !old_text.is_char_boundary(old_start) {
        old_start = old_start
            .checked_sub(1)
            .ok_or(TextSpanSelectionError::InvalidWidenedRange)?;
        new_start = new_start
            .checked_sub(1)
            .ok_or(TextSpanSelectionError::InvalidWidenedRange)?;
    }
    while !old_text.is_char_boundary(old_end) {
        old_end = old_end
            .checked_add(1)
            .ok_or(TextSpanSelectionError::InvalidWidenedRange)?;
        new_end = new_end
            .checked_add(1)
            .ok_or(TextSpanSelectionError::InvalidWidenedRange)?;
        if old_end > old.len() || new_end > new.len() {
            return Err(TextSpanSelectionError::InvalidWidenedRange);
        }
    }

    if old_start > old_end
        || old_end > old.len()
        || new_start > new_end
        || new_end > new.len()
        || !old_text.is_char_boundary(old_start)
        || !old_text.is_char_boundary(old_end)
        || !new_text.is_char_boundary(new_start)
        || !new_text.is_char_boundary(new_end)
    {
        return Err(TextSpanSelectionError::InvalidWidenedRange);
    }

    let old_span_text = old
        .get(old_start..old_end)
        .ok_or(TextSpanSelectionError::InvalidWidenedRange)?
        .to_vec();
    let replacement_text = new
        .get(new_start..new_end)
        .ok_or(TextSpanSelectionError::InvalidWidenedRange)?
        .to_vec();
    Ok(Some(SelectedSpan {
        old_start,
        old_end,
        new_start,
        new_end,
        old_span_text,
        replacement_text,
    }))
}

/// Select and identify one deterministic arbitrary text span for authoring (RFC 134 §8, v2 --
/// content-unique identity). All new authoring goes through this; v1 (`dup_index`-positional)
/// identity is never minted going forward, only ever resolved (frozen, `locate_text_span`).
pub fn plan_authored_text_span(
    old: &[u8],
    new: &[u8],
    node_id: NodeId,
) -> Result<Option<AuthoredTextSpan>, TextSpanSelectionError> {
    let Some(SelectedSpan {
        old_start,
        old_end,
        new_start,
        new_end,
        old_span_text,
        replacement_text,
    }) = select_span(old, new)?
    else {
        return Ok(None);
    };

    let old_span_hash = text_span_hash(&old_span_text);
    let (left_anchor_len, right_anchor_len) =
        choose_anchor_lengths_v2(old, &old_span_text, old_start, old_end)?;
    let left_anchor_hash = left_anchor_v2(old, old_start, left_anchor_len);
    let right_anchor_hash = right_anchor_v2(old, old_end, right_anchor_len);
    let span_id = compute_span_id_v2(
        node_id,
        &old_span_hash,
        &left_anchor_hash,
        &right_anchor_hash,
        left_anchor_len,
        right_anchor_len,
    );

    debug_assert_eq!(
        locate_text_span_v2(
            old,
            &old_span_text,
            &left_anchor_hash,
            &right_anchor_hash,
            &span_id,
            node_id,
            &old_span_hash,
            left_anchor_len,
            right_anchor_len,
        ),
        Ok((old_start, old_end))
    );

    Ok(Some(AuthoredTextSpan {
        old_start,
        old_end,
        new_start,
        new_end,
        old_span_text,
        replacement_text,
        old_span_hash,
        left_anchor_hash,
        right_anchor_hash,
        left_anchor_len,
        right_anchor_len,
        span_id,
    }))
}

/// A v1 (`dup_index`-positional) authoring plan. **Test-only** (RFC 134 §8 handoff trap #2): kept
/// so the Property B generator (`patch_algebra/tests/fixtures.rs::edit_text`) keeps building
/// v1-shaped operations unchanged -- the generator moving to v2 is a separate, deliberate future
/// step, not a side effect of this increment. No production code path uses this.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AuthoredTextSpanV1 {
    /// Byte offset where the replaced span begins in the old text.
    pub old_start: usize,
    /// Byte offset where the replaced span ends in the old text.
    pub old_end: usize,
    /// Byte offset where the replacement begins in the new text.
    pub new_start: usize,
    /// Byte offset where the replacement ends in the new text.
    pub new_end: usize,
    /// The span's bytes before the edit.
    pub old_span_text: Vec<u8>,
    /// The bytes the span becomes.
    pub replacement_text: Vec<u8>,
    /// Hash of the span's text before the edit.
    pub old_span_hash: [u8; 32],
    /// Hash of the bounded context immediately left of the span.
    pub left_anchor_hash: [u8; 32],
    /// Hash of the bounded context immediately right of the span.
    pub right_anchor_hash: [u8; 32],
    /// The v1 positional span identity this plan mints.
    pub span_id: [u8; 32],
}

/// v1 counterpart of [`plan_authored_text_span`]. See [`AuthoredTextSpanV1`].
///
/// RFC 149 §6c: reachable under the feature so `bundle`'s v1 fixture can move with its test.
#[cfg(any(test, feature = "test-support"))]
pub fn plan_authored_text_span_v1(
    old: &[u8],
    new: &[u8],
    node_id: NodeId,
) -> Result<Option<AuthoredTextSpanV1>, TextSpanSelectionError> {
    let Some(SelectedSpan {
        old_start,
        old_end,
        new_start,
        new_end,
        old_span_text,
        replacement_text,
    }) = select_span(old, new)?
    else {
        return Ok(None);
    };

    let left_anchor_hash = left_anchor(old, old_start);
    let right_anchor_hash = right_anchor(old, old_end);
    let old_span_hash = text_span_hash(&old_span_text);
    let dup_index = anchor_filtered_dup_index_v1(
        old,
        &old_span_text,
        old_start,
        old_end,
        &left_anchor_hash,
        &right_anchor_hash,
    );
    let span_id = compute_span_id(
        node_id,
        &old_span_hash,
        &left_anchor_hash,
        &right_anchor_hash,
        dup_index,
    );

    debug_assert_eq!(
        locate_text_span(
            old,
            &old_span_text,
            &left_anchor_hash,
            &right_anchor_hash,
            &span_id,
            node_id,
            &old_span_hash,
        ),
        Ok((old_start, old_end))
    );

    Ok(Some(AuthoredTextSpanV1 {
        old_start,
        old_end,
        new_start,
        new_end,
        old_span_text,
        replacement_text,
        old_span_hash,
        left_anchor_hash,
        right_anchor_hash,
        span_id,
    }))
}

/// v1's zero-based index of `(selected_start, selected_end)` within `text`'s anchor-matching
/// occurrences of `old_span_text` (test-only; see [`plan_authored_text_span_v1`]). Never returns an
/// error: the selected range is always itself anchor-matching, by construction.
// RFC 149 §6c: private, and compiled wherever its only caller `plan_authored_text_span_v1` is.
#[cfg(any(test, feature = "test-support"))]
fn anchor_filtered_dup_index_v1(
    text: &[u8],
    old_span_text: &[u8],
    selected_start: usize,
    selected_end: usize,
    left: &[u8; 32],
    right: &[u8; 32],
) -> u32 {
    let mut dup_index = 0_u32;
    let span_len = old_span_text.len();
    for start in occurrences(text, old_span_text) {
        let end = start + span_len;
        if left_anchor(text, start) == *left && right_anchor(text, end) == *right {
            if start == selected_start && end == selected_end {
                return dup_index;
            }
            dup_index += 1;
        }
    }
    unreachable!("selected span is always its own anchor-matching occurrence")
}

/// Choose the smallest anchor lengths (RFC 134 §8, each `>= TEXT_ANCHOR_WINDOW`) that make
/// `(selected_start, selected_end)` the only occurrence of `old_span_text` in `text` whose anchors
/// at that length match. **Always succeeds for a finite file** (§8.3): once `len` reaches `text.len()`,
/// every occurrence's context is clamped to its own distance from the buffer edge, and distinct
/// positions necessarily produce distinct-length -- hence distinct-hash -- contexts, so uniqueness
/// is reached at the latest when `len == text.len()`. Grown by doubling for `O(log text.len())`
/// candidate lengths, not because a smaller step would be incorrect.
pub(crate) fn choose_anchor_lengths_v2(
    text: &[u8],
    old_span_text: &[u8],
    selected_start: usize,
    selected_end: usize,
) -> Result<(u32, u32), TextSpanSelectionError> {
    let ceiling = text.len() as u32;
    let mut len = TEXT_ANCHOR_WINDOW as u32;
    loop {
        let left = left_anchor_v2(text, selected_start, len);
        let right = right_anchor_v2(text, selected_end, len);
        let span_len = old_span_text.len();
        let match_count = occurrences(text, old_span_text)
            .into_iter()
            .filter(|&start| {
                let end = start + span_len;
                left_anchor_v2(text, start, len) == left && right_anchor_v2(text, end, len) == right
            })
            .count();
        if match_count == 1 {
            return Ok((len, len));
        }
        if len >= ceiling {
            return Err(TextSpanSelectionError::AnchorUniquenessUnreachable);
        }
        len = len.saturating_mul(2).min(ceiling).max(len + 1);
    }
}

fn common_prefix_len(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right.iter())
        .take_while(|(a, b)| a == b)
        .count()
}

fn common_suffix_len(left: &[u8], right: &[u8], prefix: usize) -> usize {
    let max = left.len().min(right.len()).saturating_sub(prefix);
    left.iter()
        .rev()
        .zip(right.iter().rev())
        .take(max)
        .take_while(|(a, b)| a == b)
        .count()
}

/// RFC 134 §8.3's uniqueness-always-achievable claim, checked against inputs designed to stress
/// it, not merely trusted from the proof. If any of these ever return `Err`, that refutes §8.3 --
/// per the handoff, the design needs revisiting, not a workaround.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
mod uniqueness_stress_tests {
    use super::*;
    use prikk_object::NodeId;

    fn nid(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    /// A single-byte edit in the middle of a 500-byte buffer of one repeated character: every
    /// position in the buffer is textually identical, and a wide band around the edit shares
    /// identical 64-byte anchors too, forcing anchor growth well past `TEXT_ANCHOR_WINDOW`.
    #[test]
    fn uniform_buffer_single_byte_edit_in_the_middle() {
        let old = vec![b'a'; 500];
        let mut new = old.clone();
        new[250] = b'b';
        let plan = plan_authored_text_span(&old, &new, nid(0x60))
            .expect("uniqueness must be achievable (§8.3)")
            .expect("changed text yields a span");
        assert!(
            plan.left_anchor_len > TEXT_ANCHOR_WINDOW as u32
                || plan.right_anchor_len > TEXT_ANCHOR_WINDOW as u32,
            "this input is designed to force growth past the fixed v1 window; growth of neither \
             anchor would mean the stress case is not exercising the interesting path (got left={}, \
             right={})",
            plan.left_anchor_len,
            plan.right_anchor_len
        );
    }

    /// The same uniform-buffer pressure, but the edit sits one byte past the front of a 300-byte
    /// buffer -- the asymmetric case where the left anchor saturates at the buffer edge almost
    /// immediately while the right anchor must still grow to disambiguate.
    #[test]
    fn uniform_buffer_edit_near_the_start() {
        let mut old = vec![b'x'; 300];
        old[1] = b'a';
        let mut new = old.clone();
        new[1] = b'z';
        let plan = plan_authored_text_span(&old, &new, nid(0x61))
            .expect("uniqueness must be achievable (§8.3)")
            .expect("changed text yields a span");
        assert_eq!((plan.old_start, plan.old_end), (1, 2));
    }

    /// Mirror of the above at the tail end of the buffer.
    #[test]
    fn uniform_buffer_edit_near_the_end() {
        let mut old = vec![b'x'; 300];
        old[298] = b'a';
        let mut new = old.clone();
        new[298] = b'z';
        let plan = plan_authored_text_span(&old, &new, nid(0x62))
            .expect("uniqueness must be achievable (§8.3)")
            .expect("changed text yields a span");
        assert_eq!((plan.old_start, plan.old_end), (298, 299));
    }

    /// A repeated *multi-byte* span (not a single byte) in an otherwise-uniform buffer -- stresses
    /// the occurrence scan finding many candidates for a wider needle, not just many single-byte
    /// positions.
    #[test]
    fn uniform_buffer_repeated_multibyte_span() {
        let mut old = Vec::new();
        for _ in 0..20 {
            old.extend_from_slice(b"pqpqpqpqpqpqpqpqpqpqpqpqpqpqpqpq"); // 32 uniform-ish bytes
        }
        let mid = old.len() / 2;
        let mut new = old.clone();
        new[mid..mid + 2].copy_from_slice(b"ZZ");
        let plan = plan_authored_text_span(&old, &new, nid(0x63))
            .expect("uniqueness must be achievable (§8.3)")
            .expect("changed text yields a span");
        // Round-trip: the plan must actually localize uniquely and reproduce `new` when spliced.
        let (start, end) = locate_text_span_v2(
            &old,
            &plan.old_span_text,
            &plan.left_anchor_hash,
            &plan.right_anchor_hash,
            &plan.span_id,
            nid(0x63),
            &plan.old_span_hash,
            plan.left_anchor_len,
            plan.right_anchor_len,
        )
        .expect("plan must localize uniquely");
        let spliced = splice_text(&old, start, end, &plan.replacement_text).expect("splice");
        assert_eq!(spliced, new);
    }
}
