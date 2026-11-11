//! RFC 125 §3: `validate_repo_path` previously had no length check of any kind, so an
//! over-long path entered signed history and only failed later, at checkout, as a raw OS error
//! (`NAME_MAX`/`MAX_PATH`) on a repository that had already verified clean.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use super::{MAX_COMPONENT_LEN, MAX_TOTAL_LEN, validate_repo_path};

fn repeat(byte: char, count: usize) -> String {
    std::iter::repeat_n(byte, count).collect()
}

#[test]
fn a_component_at_the_limit_is_accepted() {
    let path = repeat('a', MAX_COMPONENT_LEN);
    assert!(validate_repo_path(&path).is_ok(), "{path}");
}

#[test]
fn a_component_one_byte_over_the_limit_is_refused() {
    let path = repeat('a', MAX_COMPONENT_LEN + 1);
    let err = validate_repo_path(&path).expect_err("over-length component must be refused");
    assert!(format!("{err}").contains("exceeds"), "{err}");
}

/// The total-length cap must fire even when every individual component is well within
/// [`MAX_COMPONENT_LEN`] -- many short components summing past [`MAX_TOTAL_LEN`] is exactly the
/// shape a per-component-only check would miss.
#[test]
fn many_short_components_summing_past_the_total_cap_are_refused() {
    let segment = repeat('a', 8);
    let segment_count = (MAX_TOTAL_LEN / (segment.len() + 1)) + 2;
    let path = std::iter::repeat_n(segment, segment_count)
        .collect::<Vec<_>>()
        .join("/");
    assert!(path.len() > MAX_TOTAL_LEN, "test path must exceed the cap");
    let err = validate_repo_path(&path).expect_err("over-length total path must be refused");
    assert!(format!("{err}").contains("exceeds"), "{err}");
}

#[test]
fn many_short_components_within_the_total_cap_are_accepted() {
    let segment = repeat('a', 8);
    let segment_count = 100; // 100 * 9 - 1 = 899 bytes, comfortably under MAX_TOTAL_LEN.
    let path = std::iter::repeat_n(segment, segment_count)
        .collect::<Vec<_>>()
        .join("/");
    assert!(
        path.len() < MAX_TOTAL_LEN,
        "test path must stay under the cap"
    );
    assert!(validate_repo_path(&path).is_ok(), "{path}");
}

// ---- 0.57.0 U2: repository format 8 admits non-ASCII paths; the decode table refuses a fixed set of characters ----

use super::decode_table::{refused_code_points, table_is_sorted_and_disjoint};
use super::{first_refused_character, is_windows_reserved_name, refusal_reason};
use prikk_error::{DecodeFailureKind, PrikkError};

#[test]
fn the_decode_table_is_sorted_disjoint_and_exactly_this_size() {
    assert!(table_is_sorted_and_disjoint());
    // The count is pinned: a change to the table is a change to what history is valid, and may happen only with a format
    // bump. Per class, as the table states them:
    // C0 32 + DEL 1 + C1 32 + SHY 1 + ALM 1 + ZWSP/ZWNJ/ZWJ 3 + LRM/RLM 2 + LS/PS 2 + embeddings 5 + word joiner..invisible
    // plus 5 + isolates 4 + FDD0..FDEF 32 + BOM 1 + interlinear 3 + the 34 plane-end noncharacters + 128 tags.
    let expected = 32 + 1 + 32 + 1 + 1 + 3 + 2 + 2 + 5 + 5 + 4 + 32 + 1 + 3 + 34 + 128;
    assert_eq!(refused_code_points().count(), expected);
    assert!(
        refused_code_points().all(|code| char::from_u32(code).is_some()),
        "no surrogate is listed (a surrogate is not a char and cannot be in a str)"
    );
}

#[test]
fn the_table_refuses_what_it_lists_and_admits_its_neighbours() {
    for refused in [
        '\u{0}',
        '\u{1f}',
        '\u{7f}',
        '\u{80}',
        '\u{9f}',
        '\u{ad}',
        '\u{61c}',
        '\u{200b}',
        '\u{200c}',
        '\u{200d}',
        '\u{200e}',
        '\u{200f}',
        '\u{2028}',
        '\u{2029}',
        '\u{202a}',
        '\u{202e}',
        '\u{2060}',
        '\u{2064}',
        '\u{2066}',
        '\u{2069}',
        '\u{fdd0}',
        '\u{fdef}',
        '\u{feff}',
        '\u{fff9}',
        '\u{fffb}',
        '\u{fffe}',
        '\u{ffff}',
        '\u{1fffe}',
        '\u{10ffff}',
        '\u{e0000}',
        '\u{e0041}',
        '\u{e007f}',
    ] {
        assert!(
            refusal_reason(refused).is_some(),
            "U+{:04X} is refused",
            u32::from(refused)
        );
    }
    for admitted in [
        ' ',
        'a',
        '~',
        '\u{a0}',
        '\u{a1}',
        '\u{ac}',
        '\u{ae}',
        '\u{61b}',
        '\u{61d}',
        '\u{200a}',
        '\u{2010}',
        '\u{2027}',
        '\u{202f}',
        '\u{205f}',
        '\u{2065}',
        '\u{206a}',
        '\u{206f}',
        '\u{fdcf}',
        '\u{fdf0}',
        '\u{fefe}',
        '\u{ff00}',
        '\u{fff8}',
        '\u{fffc}',
        '\u{fffd}',
        '\u{10000}',
        '\u{1fffd}',
        '\u{e0080}',
        '\u{10fffd}',
        '\u{e9}',
        '\u{65e5}',
        '\u{d55c}',
        '\u{1f600}',
    ] {
        assert!(
            refusal_reason(admitted).is_none(),
            "U+{:04X} is admitted",
            u32::from(admitted)
        );
    }
    assert_eq!(
        refusal_reason('\u{200d}'),
        Some("invisible format character")
    );
    assert_eq!(refusal_reason('\u{202e}'), Some("bidirectional control"));
    assert_eq!(refusal_reason('\u{85}'), Some("control"));
    assert_eq!(refusal_reason('\u{fffe}'), Some("noncharacter"));
    assert_eq!(
        refusal_reason('\u{2028}'),
        Some("line or paragraph separator")
    );
    assert_eq!(
        first_refused_character("ab\u{200d}c\u{202e}"),
        Some(('\u{200d}', "invisible format character"))
    );
    assert_eq!(first_refused_character("plain"), None);
}

#[test]
fn format_8_admits_non_ascii_paths_in_either_normalization_form() {
    // Decode does not ask for NFC (ruling C3): both spellings of one visible name are valid history.
    for accepted in [
        "pages/\u{65e5}\u{672c}\u{8a9e}.md",
        "caf\u{e9}.md",
        "cafe\u{301}.md",
        "\u{d55c}\u{ae00}/\u{bb38}\u{c11c}.txt",
        "emoji/\u{1f600}.txt",
        "\u{a0}leading-nbsp.txt",
    ] {
        assert!(
            validate_repo_path(accepted).is_ok(),
            "{accepted:?} validates"
        );
    }
}

#[test]
fn a_refused_character_is_outside_the_version_rule_naming_the_character_never_damage() {
    let err = validate_repo_path("pages/a\u{200d}b.md").expect_err("ZWJ is refused");
    assert!(
        matches!(
            &err,
            PrikkError::RefusedPathCharacter { character: '\u{200d}', reason: "invisible format character", path }
                if path == "pages/a\u{200d}b.md"
        ),
        "{err:?}"
    );
    assert_eq!(
        err.decode_failure_kind(),
        DecodeFailureKind::OutsideVersionRule
    );
    let said = err.to_string();
    assert!(
        said.contains("U+200D (invisible format character)"),
        "{said}"
    );
    // The path is written with the invisible character escaped; printed raw it would read `pages/ab.md`.
    assert!(said.contains("pages/a\\u{200d}b.md"), "{said}");
    assert!(!said.contains('\u{200d}'), "{said}");
    assert!(
        said.contains("outside what this version of prikk accepts"),
        "{said}"
    );
    assert!(
        said.contains("a newer version of prikk may have written it"),
        "{said}"
    );
    // ZWNJ and the C1 and bidi examples.
    for refused in [
        "a\u{200c}b",
        "a\u{85}b",
        "a\u{202e}b",
        "a\u{fffe}b",
        "a\u{feff}b",
    ] {
        let err = validate_repo_path(refused).expect_err("refused");
        assert_eq!(
            err.decode_failure_kind(),
            DecodeFailureKind::OutsideVersionRule,
            "{refused:?}"
        );
    }
    // A C0 control stays what it always was: permanently invalid, plain `InvalidName`.
    let err = validate_repo_path("a\u{1}b").expect_err("refused");
    assert!(matches!(err, PrikkError::InvalidName(_)), "{err:?}");
    assert_eq!(err.decode_failure_kind(), DecodeFailureKind::Corrupt);
}

#[test]
fn the_windows_superscript_device_names_are_reserved_in_every_case_and_with_an_extension() {
    for reserved in [
        "COM\u{b9}",
        "com\u{b2}.txt",
        "Com\u{b3}.tar.gz",
        "LPT\u{b9}",
        "lpt\u{b2}",
        "LPT\u{b3}.log",
    ] {
        assert!(is_windows_reserved_name(reserved), "{reserved:?}");
        let err = validate_repo_path(&format!("dir/{reserved}")).expect_err("reserved");
        assert!(matches!(err, PrikkError::InvalidName(_)), "{err:?}");
    }
    for fine in [
        "COM0",
        "COM10",
        "COM\u{b9}x",
        "COM\u{2081}",
        "console",
        "LPT\u{b4}",
    ] {
        assert!(!is_windows_reserved_name(fine), "{fine:?}");
    }
}

#[test]
fn a_component_is_bounded_in_bytes_and_so_in_utf16_units() {
    // 85 three-byte characters is 255 bytes (accepted); 86 is 258 (refused). UTF-16 length is 85 and 86: never over the
    // byte limit while under it, which is why there is no second, UTF-16 limit.
    let at_limit: String = std::iter::repeat_n('\u{65e5}', 85).collect();
    assert_eq!(at_limit.len(), 255);
    assert!(at_limit.encode_utf16().count() <= at_limit.len());
    assert!(validate_repo_path(&at_limit).is_ok());
    let over: String = std::iter::repeat_n('\u{65e5}', 86).collect();
    assert!(validate_repo_path(&over).is_err());
    // The property the dropped check relied on, over every width: UTF-16 units never exceed UTF-8 bytes.
    for character in ['a', '\u{e9}', '\u{65e5}', '\u{1f600}'] {
        assert!(character.len_utf16() <= character.len_utf8());
    }
}

// ---- 0.57.0 U3: the `.prikk` check folds ----

#[test]
fn the_prikk_metadata_directory_is_refused_under_unicode_case_folding_as_first_component_only() {
    // On a case-insensitive, normalizing file system these are the `.prikk` directory (the Kelvin sign folds to `k`).
    for refused in [
        ".prikk/x",
        ".PRIKK/x",
        ".Prikk",
        ".pri\u{212a}k/x",
        ".PRI\u{212a}K",
    ] {
        let err = validate_repo_path(refused).expect_err(refused);
        assert!(
            matches!(&err, PrikkError::InvalidName(text) if text.contains(".prikk metadata directory")),
            "{refused:?}: {err:?}"
        );
    }
    // Only the first component; and a different name is a different name.
    for accepted in [
        "sub/.prikk",
        "sub/.pri\u{212a}k",
        ".prikk.md",
        ".prikk\u{301}/x",
        ".pri/x",
        ".prikks",
    ] {
        assert!(validate_repo_path(accepted).is_ok(), "{accepted:?}");
    }
}

#[test]
fn the_canonical_key_of_an_ascii_string_is_the_same_by_the_fast_path_and_the_full_computation() {
    use unicode_normalization::UnicodeNormalization;
    let full = |value: &str| -> String {
        let decomposed: String = value.nfd().collect();
        let folded = unicase::UniCase::unicode(decomposed.as_str()).to_folded_case();
        folded.nfd().collect()
    };
    for code in 0u8..128 {
        let single = char::from(code).to_string();
        assert_eq!(
            super::canonical_caseless_key(&single),
            full(&single),
            "U+{code:04X}"
        );
    }
    for sample in [
        "README.md",
        "src/Main.RS",
        ".PRIKK",
        "A-b_C.1",
        "",
        "zzzZZZ",
    ] {
        assert_eq!(
            super::canonical_caseless_key(sample),
            full(sample),
            "{sample:?}"
        );
    }
}
