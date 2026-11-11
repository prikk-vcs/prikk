//! Behaviour pins for the two Unicode crates (owner decision D2, option B; 0.57.0 U2). A crate upgrade that changes NFC,
//! folding, or the Unicode version either keeps these green or moves them **with a reason** -- it does not pass silently.
//!
//! What is pinned:
//! - the Unicode version both crates ship (18.0.0), so the two cannot drift apart;
//! - NFC/NFD on known strings, including the singleton, the ligature that is NFC, and every Hangul syllable (the one place
//!   `unicode-normalization` uses `unsafe`: five `char::from_u32_unchecked` sites, accepted in the U0 review on the
//!   strength of this exhaustive test);
//! - full case folding and the canonical caseless key on known strings, including the deliberate over-refusals.

#![allow(clippy::expect_used)]

use unicode_normalization::UnicodeNormalization;

use super::{canonical_caseless_key, non_nfc_spelling};

fn nfc(value: &str) -> String {
    value.nfc().collect()
}

fn nfd(value: &str) -> String {
    value.nfd().collect()
}

#[test]
fn both_crates_ship_the_unicode_version_the_table_was_chosen_against() {
    // `unicode-normalization` exposes its version. `unicase` has no constant; `unicase_folds_exactly_the_unicode_18_
    // case_folding_rows_this_file_pins` below pins its table by behaviour.
    assert_eq!(unicode_normalization::UNICODE_VERSION, (18, 0, 0));
}

#[test]
fn nfc_on_known_strings() {
    // Composition.
    assert_eq!(nfc("e\u{301}"), "\u{e9}");
    assert_eq!(nfc("cafe\u{301}"), "caf\u{e9}");
    // Decomposition.
    assert_eq!(nfd("\u{e9}"), "e\u{301}");
    // A singleton decomposition: the Angstrom sign is canonically the letter A with ring above.
    assert_eq!(nfc("\u{212B}"), "\u{c5}");
    // A compatibility-only ligature is already NFC (NFC does not apply compatibility mappings).
    assert_eq!(nfc("\u{fb01}"), "\u{fb01}");
    // Canonical ordering of combining marks: dot below (ccc 220) sorts before dot above (ccc 230), then composes.
    assert_eq!(nfc("a\u{307}\u{323}"), "\u{1ea1}\u{307}");
    // Already NFC, ASCII, and CJK are unchanged.
    for unchanged in [
        "plain.txt",
        "\u{65e5}\u{672c}\u{8a9e}.md",
        "\u{d55c}\u{ae00}",
    ] {
        assert_eq!(nfc(unchanged), unchanged);
    }
}

#[test]
fn non_nfc_spelling_names_the_nfc_form_or_nothing() {
    assert_eq!(non_nfc_spelling("caf\u{e9}"), None);
    assert_eq!(
        non_nfc_spelling("cafe\u{301}").as_deref(),
        Some("caf\u{e9}")
    );
    assert_eq!(
        non_nfc_spelling("\u{212B}").as_deref(),
        Some("\u{c5}"),
        "a singleton is not NFC"
    );
    assert_eq!(non_nfc_spelling("plain.txt"), None);
    assert_eq!(non_nfc_spelling(""), None);
    // A string already in NFC that carries a combining mark with no base to compose with.
    assert_eq!(non_nfc_spelling("\u{301}"), None);
}

/// **Every Hangul syllable** (11,172 of them), and every jamo pair that composes. This is the whole input space of the
/// five `unsafe` sites in `unicode-normalization` (`char::from_u32_unchecked` in Hangul decomposition and composition);
/// the U0 review accepted the crate on the strength of reading those sites and of this test.
#[test]
fn every_hangul_syllable_decomposes_and_recomposes_and_every_jamo_pair_composes() {
    const S_BASE: u32 = 0xAC00;
    const L_BASE: u32 = 0x1100;
    const V_BASE: u32 = 0x1161;
    const T_BASE: u32 = 0x11A7;
    const L_COUNT: u32 = 19;
    const V_COUNT: u32 = 21;
    const T_COUNT: u32 = 28;
    const S_COUNT: u32 = L_COUNT * V_COUNT * T_COUNT;
    assert_eq!(S_COUNT, 11_172);
    let scalar = |code: u32| char::from_u32(code).expect("a scalar value");

    for index in 0..S_COUNT {
        let syllable = scalar(S_BASE + index);
        let l = scalar(L_BASE + index / (V_COUNT * T_COUNT));
        let v = scalar(V_BASE + (index % (V_COUNT * T_COUNT)) / T_COUNT);
        let t = index % T_COUNT;
        let mut expected = String::new();
        expected.push(l);
        expected.push(v);
        if t != 0 {
            expected.push(scalar(T_BASE + t));
        }
        let decomposed = nfd(&syllable.to_string());
        assert_eq!(
            decomposed,
            expected,
            "decomposition of U+{:04X}",
            S_BASE + index
        );
        assert_eq!(
            nfc(&decomposed),
            syllable.to_string(),
            "NFC(NFD(s)) == s for U+{:04X}",
            S_BASE + index
        );
        assert!(
            non_nfc_spelling(&syllable.to_string()).is_none(),
            "a precomposed syllable is NFC"
        );
        assert_eq!(
            non_nfc_spelling(&decomposed).as_deref(),
            Some(syllable.to_string().as_str()),
            "the jamo spelling is not NFC and names the syllable"
        );
    }

    // Every L+V pair composes to the LV syllable.
    for l in 0..L_COUNT {
        for v in 0..V_COUNT {
            let pair = format!("{}{}", scalar(L_BASE + l), scalar(V_BASE + v));
            let expected = scalar(S_BASE + (l * V_COUNT + v) * T_COUNT);
            assert_eq!(nfc(&pair), expected.to_string(), "L{l}+V{v}");
        }
    }
    // Every LV syllable + T (1..=27) composes to the LVT syllable; a T after an LVT syllable composes with nothing.
    for lv in 0..(L_COUNT * V_COUNT) {
        let lv_syllable = scalar(S_BASE + lv * T_COUNT);
        for t in 1..T_COUNT {
            let with_trailing = format!("{lv_syllable}{}", scalar(T_BASE + t));
            let expected = scalar(S_BASE + lv * T_COUNT + t);
            assert_eq!(nfc(&with_trailing), expected.to_string(), "LV{lv}+T{t}");
            let twice = format!("{expected}{}", scalar(T_BASE + t));
            assert_eq!(nfc(&twice), twice, "an LVT syllable takes no further T");
        }
    }
}

#[test]
fn full_case_folding_and_the_canonical_caseless_key_on_known_strings() {
    let same = [
        // Full folding: one scalar folds to two.
        ("stra\u{df}e", "strasse"),
        ("Ma\u{df}e", "MASSE"),
        ("\u{1e9e}", "ss"),
        ("\u{fb03}", "FFI"),
        // Capital I with dot above folds to i + combining dot above.
        ("\u{130}", "i\u{307}"),
        // Final and medial sigma.
        ("\u{3c3}", "\u{3c2}"),
        // The Kelvin sign folds to k, and so to the ASCII letter.
        ("\u{212A}", "k"),
        (".pri\u{212A}k", ".prikk"),
        // Canonical equivalence needs the leading NFD: e + acute equals the precomposed capital E-acute only so.
        ("e\u{301}", "\u{c9}"),
        ("\u{c9}", "E\u{301}"),
        ("cafe\u{301}", "CAF\u{c9}"),
        // Why the **leading** NFD is part of the key: the combining ypogegrammeni (U+0345, ccc 240) folds to a letter (ccc 0),
        // so the order of two marks that canonical ordering would settle is lost if folding comes first. These two spellings
        // are canonically equivalent; only NFD-fold-NFD sees it.
        ("a\u{345}\u{301}", "a\u{301}\u{345}"),
        // Hangul: a syllable and its jamo spelling.
        ("\u{ac01}", "\u{1100}\u{1161}\u{11a8}"),
    ];
    for (left, right) in same {
        assert_eq!(
            canonical_caseless_key(left),
            canonical_caseless_key(right),
            "{left:?} and {right:?} collide"
        );
    }
    let different = [
        ("a", "b"),
        ("e", "\u{e9}"),
        ("ss", "s"),
        ("\u{df}", "s"),
        ("file", "files"),
        ("\u{65e5}\u{672c}", "\u{65e5}\u{672c}\u{8a9e}"),
    ];
    for (left, right) in different {
        assert_ne!(
            canonical_caseless_key(left),
            canonical_caseless_key(right),
            "{left:?} and {right:?} are different names"
        );
    }
    // The key itself, for the strings the reference states.
    assert_eq!(canonical_caseless_key("Stra\u{df}e"), "strasse");
    assert_eq!(canonical_caseless_key("README.md"), "readme.md");
}

/// `unicase`'s fold, row by row, for the multi-scalar (`F`) mappings `CaseFolding.txt` lists for Unicode 18.0.0 that matter
/// to a filename: the ones U0 measured against the whole file (all 1,112,064 scalars, 0 mismatches). A sample of the rows,
/// pinned here so a crate upgrade that changed one is seen.
#[test]
fn unicase_folds_exactly_the_unicode_18_case_folding_rows_this_file_pins() {
    let rows: [(&str, &str); 10] = [
        ("\u{df}", "ss"),
        ("\u{130}", "i\u{307}"),
        ("\u{149}", "\u{2bc}n"),
        ("\u{1f0}", "j\u{30c}"),
        ("\u{390}", "\u{3b9}\u{308}\u{301}"),
        ("\u{587}", "\u{565}\u{582}"),
        ("\u{1e96}", "h\u{331}"),
        ("\u{1e9e}", "ss"),
        ("\u{fb00}", "ff"),
        ("\u{fb06}", "st"),
    ];
    for (input, folded) in rows {
        let got = unicase::UniCase::unicode(input).to_folded_case();
        assert_eq!(got, folded, "fold of {input:?}");
    }
}
