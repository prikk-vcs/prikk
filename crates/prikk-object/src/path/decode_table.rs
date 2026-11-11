//! The decode table (0.57.0 U2): the characters a repository path may never contain, **as an explicit list in this
//! file**, frozen at repository format 8.
//!
//! **Why a list and not a Unicode property.** A decode-time refusal decides whether stored history is valid, so it must
//! mean the same thing to every binary that reads the history (ruling C3). The Unicode data does not hold still:
//! `General_Category=Cf` had 131 code points in Unicode 3.2 and 170 in 16.0 (U+180E left `Zs` for `Cf`), so "is a format
//! character" asked at decode would let a newer prikk refuse history an older one accepted. Neither dependency crate
//! carries general-category data either. So each entry below was chosen by us, from the property it belongs to today, and
//! **the table may grow only with a format bump**. Loosening it later is possible (a newer format may admit more);
//! tightening it is not, because history already accepted would become refused.
//!
//! NFC is **not** in this table. It is checked only where a name is authored (`commit`), never at decode (the NFC status of
//! a name containing a code point unassigned today can change in a later Unicode version): a documented exception to
//! DC-54's encode/decode symmetry (`docs/src/reference/path-safety.md`).
//!
//! Sources (the Unicode Character Encoding Stability Policy and the property files, rechecked in U0): `Cc` is immutable
//! (C0 and C1: 65 code points in 3.2 and in 16.0); `Noncharacter_Code_Point` is an immutable property; `Zl` and `Zp` have one
//! member each; the twelve bidirectional controls are the same twelve in 16.0; the format characters below are an explicit
//! selection, not "all of `Cf`".

/// One refused range, inclusive, and the name its sentence uses.
struct Range {
    first: u32,
    last: u32,
    reason: &'static str,
}

const fn range(first: u32, last: u32, reason: &'static str) -> Range {
    Range {
        first,
        last,
        reason,
    }
}

const CONTROL: &str = "control";
const NONCHARACTER: &str = "noncharacter";
const SEPARATOR: &str = "line or paragraph separator";
const BIDI: &str = "bidirectional control";
const FORMAT: &str = "invisible format character";

/// The table. Ascending by `first`, non-overlapping: the unit test below checks both, so a lookup can be a plain scan and a
/// reader can audit the list top to bottom.
const TABLE: &[Range] = &[
    // C0 controls and DEL (already refused before format 8; ASCII).
    range(0x0000, 0x001F, CONTROL),
    range(0x007F, 0x007F, CONTROL),
    // C1 controls.
    range(0x0080, 0x009F, CONTROL),
    // Soft hyphen: invisible in most renderings.
    range(0x00AD, 0x00AD, FORMAT),
    // Arabic letter mark (a bidirectional control).
    range(0x061C, 0x061C, BIDI),
    // Zero width space, ZWNJ, ZWJ. ZWNJ is needed to spell Persian and some Indic scripts correctly, and ZWJ forms many
    // emoji sequences: refused on purpose (owner, 2026-11-11), revisited if a consumer asks. Two names that differ only by
    // an invisible character would look identical.
    range(0x200B, 0x200D, FORMAT),
    // Left-to-right mark, right-to-left mark.
    range(0x200E, 0x200F, BIDI),
    // Line separator and paragraph separator (`Zl`, `Zp`).
    range(0x2028, 0x2029, SEPARATOR),
    // Embeddings and overrides: LRE, RLE, PDF, LRO, RLO.
    range(0x202A, 0x202E, BIDI),
    // Word joiner, function application, invisible times, separator, plus.
    range(0x2060, 0x2064, FORMAT),
    // Isolates: LRI, RLI, FSI, PDI.
    range(0x2066, 0x2069, BIDI),
    // Arabic-script noncharacters.
    range(0xFDD0, 0xFDEF, NONCHARACTER),
    // Byte order mark / zero width no-break space.
    range(0xFEFF, 0xFEFF, FORMAT),
    // Interlinear annotation anchor, separator, terminator.
    range(0xFFF9, 0xFFFB, FORMAT),
    // The last two code points of the basic multilingual plane.
    range(0xFFFE, 0xFFFF, NONCHARACTER),
    // The last two code points of each of the sixteen supplementary planes.
    range(0x1FFFE, 0x1FFFF, NONCHARACTER),
    range(0x2FFFE, 0x2FFFF, NONCHARACTER),
    range(0x3FFFE, 0x3FFFF, NONCHARACTER),
    range(0x4FFFE, 0x4FFFF, NONCHARACTER),
    range(0x5FFFE, 0x5FFFF, NONCHARACTER),
    range(0x6FFFE, 0x6FFFF, NONCHARACTER),
    range(0x7FFFE, 0x7FFFF, NONCHARACTER),
    range(0x8FFFE, 0x8FFFF, NONCHARACTER),
    range(0x9FFFE, 0x9FFFF, NONCHARACTER),
    range(0xAFFFE, 0xAFFFF, NONCHARACTER),
    range(0xBFFFE, 0xBFFFF, NONCHARACTER),
    range(0xCFFFE, 0xCFFFF, NONCHARACTER),
    range(0xDFFFE, 0xDFFFF, NONCHARACTER),
    // The Tags block (language tags and the emoji tag sequences' invisible letters).
    range(0xE0000, 0xE007F, FORMAT),
    range(0xEFFFE, 0xEFFFF, NONCHARACTER),
    range(0xFFFFE, 0xFFFFF, NONCHARACTER),
    range(0x10FFFE, 0x10FFFF, NONCHARACTER),
];

/// Why `character` is refused, or `None` when the table admits it.
#[must_use]
pub fn refusal_reason(character: char) -> Option<&'static str> {
    let code = u32::from(character);
    TABLE
        .iter()
        .find(|entry| entry.first <= code && code <= entry.last)
        .map(|entry| entry.reason)
}

/// The first character of `value` that the table refuses, with why; `None` when `value` has none.
#[must_use]
pub fn first_refused_character(value: &str) -> Option<(char, &'static str)> {
    value
        .chars()
        .find_map(|character| refusal_reason(character).map(|reason| (character, reason)))
}

/// Every refused code point, in ascending order (for the tests that pin the table).
#[cfg(test)]
pub(super) fn refused_code_points() -> impl Iterator<Item = u32> {
    TABLE.iter().flat_map(|entry| entry.first..=entry.last)
}

#[cfg(test)]
pub(super) fn table_is_sorted_and_disjoint() -> bool {
    TABLE
        .windows(2)
        .all(|pair| matches!(pair, [a, b] if a.last < b.first))
        && TABLE.iter().all(|entry| entry.first <= entry.last)
}
