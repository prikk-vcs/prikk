//! Handoff §6 control 6: the committed prikk profile round-trips -- parsed, and the parse agrees
//! with the file. This is the check that the format is actually readable by the thing that will
//! read it, before increment 2 depends on it.
//!
//! RFC 139 §9 increment 4 adds a second committed profile, `sindresorhus-awesome.toml` -- the same
//! control applies to it, unchanged, so it is checked here rather than skipped as "the format
//! already proved itself once."

#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;

const COMMITTED_PROFILE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/profiles/prikk-self.toml"
));

const COMMITTED_SECOND_PROFILE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/profiles/sindresorhus-awesome.toml"
));

fn assert_round_trips(text: &str, expected_source_repository: &str) -> Profile {
    let parsed: Profile = toml::from_str(text)
        .expect("the committed profile must parse as this crate's own Profile type");
    assert_eq!(parsed.schema_version, SCHEMA_VERSION);
    assert_eq!(
        parsed.provenance.source_repository,
        expected_source_repository
    );
    assert!(
        !parsed.provenance.extraction_commands.is_empty(),
        "provenance must name at least one extraction command"
    );
    assert!(parsed.shape.commit_count > 0);

    let re_rendered =
        toml::to_string_pretty(&parsed).expect("a parsed profile must re-render as TOML");
    let re_parsed: Profile = toml::from_str(&re_rendered)
        .expect("the re-rendered profile must itself parse as this crate's own Profile type");
    assert_eq!(
        parsed, re_parsed,
        "parse -> render -> parse must agree with the original parse"
    );
    parsed
}

#[test]
fn control6_the_committed_prikk_profile_round_trips() {
    assert_round_trips(COMMITTED_PROFILE, "prikk (self)");
}

/// The same control (handoff §6 control 6), applied to the second profile RFC 139 §9 increment 4
/// adds. Also checks `commit_count` matches `prikk-self.toml`'s exactly -- comparability (the
/// increment's own stated purpose) requires equal `n`, not merely a profile that parses.
#[test]
fn the_committed_second_profile_round_trips_and_matches_prikk_selfs_commit_count() {
    let prikk_self = assert_round_trips(COMMITTED_PROFILE, "prikk (self)");
    let second = assert_round_trips(
        COMMITTED_SECOND_PROFILE,
        "https://github.com/sindresorhus/awesome",
    );
    assert_eq!(
        second.shape.commit_count, prikk_self.shape.commit_count,
        "the two profiles must be extracted at the same n to be comparable"
    );
}
