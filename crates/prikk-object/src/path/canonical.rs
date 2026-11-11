//! Unicode normalization and canonical caseless comparison for repository paths (0.57.0 U2), over the two pinned crates
//! `unicode-normalization` (NFC/NFD) and `unicase` (full case folding). **One definition each, here**, so no caller
//! assembles its own from the crates and no two call sites can disagree.
//!
//! - [`non_nfc_spelling`] is for **authoring only** (`commit`): a name that is not in NFC is refused, naming its NFC spelling.
//!   It is never asked at decode (ruling C3: NFC status can change for a code point unassigned today).
//! - [`canonical_caseless_key`] is for the checks that **protect the disk** (U3: `seal`, `extract`, `checkout`): two names
//!   that have the same key are the same name to a case-insensitive or normalizing filesystem, so they must not both
//!   exist. Unicode's canonical caseless match is `NFD(fold(NFD(x)))`. A collision check may tighten across Unicode versions;
//!   refusing more is the safe direction.
//!
//! The behaviour of both is pinned by `path/unicode_stability.rs`, so a crate upgrade that changes either fails a test.

use unicase::UniCase;
use unicode_normalization::{UnicodeNormalization, is_nfc};

/// `Some(nfc)` when `value` is **not** in Unicode normalization form C, with its NFC spelling; `None` when it is.
#[must_use]
pub fn non_nfc_spelling(value: &str) -> Option<String> {
    if is_nfc(value) {
        return None;
    }
    let composed: String = value.nfc().collect();
    // `is_nfc` is a quick check that answers "no" only when the string is not in NFC, but a string that is already
    // equal to its NFC form is in NFC whatever the quick check said; return `None` then rather than a "refusal"
    // whose two spellings are the same.
    (composed != value).then_some(composed)
}

/// Unicode's canonical caseless form of `value`: `NFD(fold(NFD(value)))`, with full case folding (`straße` and `strasse`
/// have the same key, as do `ẞ` and `ss`). Two repository paths with the same key collide.
#[must_use]
pub fn canonical_caseless_key(value: &str) -> String {
    let key = unseamed_canonical_caseless_key(value);
    #[cfg(feature = "test-support")]
    let key = seam::apply(key);
    key
}

fn unseamed_canonical_caseless_key(value: &str) -> String {
    // ASCII fast path: NFD leaves ASCII alone and its full case folding is `to_ascii_lowercase`, so the key is the same
    // either way (a test checks all 128 ASCII scalars and mixed strings). Almost every path is ASCII, and `seal` asks the key
    // of every live path.
    if value.is_ascii() {
        return value.to_ascii_lowercase();
    }
    let decomposed: String = value.nfd().collect();
    let folded = UniCase::unicode(decomposed.as_str()).to_folded_case();
    folded.nfd().collect()
}

pub use prikk_error::escape_non_ascii;

/// **A stricter rule, for tests** (0.57.0 U3b). The collision rule may tighten across Unicode versions, and `verify` must then
/// warn about a sealed block that was valid when it was sealed, not fail it. No real input can show that today (the crates
/// are pinned), so a test arms this to make the key fold one more pair of characters: history sealed under the real rule is then
/// judged by a stricter one. Thread-local, restored when the guard drops, absent from a shipped build.
#[cfg(feature = "test-support")]
pub fn fold_one_more_pair_for_test(from: char, into: char) -> StricterRuleGuard {
    seam::PAIRS.with(|pairs| pairs.borrow_mut().push((from, into)));
    StricterRuleGuard(())
}

/// Disarms [`fold_one_more_pair_for_test`] when dropped.
#[cfg(feature = "test-support")]
#[derive(Debug)]
pub struct StricterRuleGuard(());

#[cfg(feature = "test-support")]
impl Drop for StricterRuleGuard {
    fn drop(&mut self) {
        seam::PAIRS.with(|pairs| {
            pairs.borrow_mut().pop();
        });
    }
}

#[cfg(feature = "test-support")]
mod seam {
    use std::cell::RefCell;

    thread_local! {
        pub(super) static PAIRS: RefCell<Vec<(char, char)>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn apply(key: String) -> String {
        PAIRS.with(|pairs| {
            let pairs = pairs.borrow();
            if pairs.is_empty() {
                return key;
            }
            key.chars()
                .map(|character| {
                    pairs
                        .iter()
                        .find(|(from, _)| *from == character)
                        .map_or(character, |(_, into)| *into)
                })
                .collect()
        })
    }
}
