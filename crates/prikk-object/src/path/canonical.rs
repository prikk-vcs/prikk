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
    let decomposed: String = value.nfd().collect();
    let folded = UniCase::unicode(decomposed.as_str()).to_folded_case();
    folded.nfd().collect()
}

/// `value` with every non-ASCII character written `\u{..}`, so two spellings that look identical on screen (an NFC and an NFD
/// `é`) can be told apart in a sentence.
#[must_use]
pub fn escape_non_ascii(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii() {
                character.to_string()
            } else {
                character.escape_unicode().to_string()
            }
        })
        .collect()
}
