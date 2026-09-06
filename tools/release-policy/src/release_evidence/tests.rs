//! RFC 141 increment 1 controls (handoff §6). Each control was seen to fail before it passed --
//! the perturbation is stated in this file's own comments, and restated in the implementation
//! report.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::schema::SchemaProfile;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

fn schema() -> SchemaProfile {
    let root = repo_root();
    let bytes = std::fs::read(root.join(NORMATIVE_SCHEMA_PATH)).unwrap();
    let value = json::parse(&bytes).unwrap();
    SchemaProfile::compile(&value).unwrap()
}

fn not_observed_tag(version: &str) -> Value {
    json!({
        "name": version,
        "object_id": "0".repeat(40),
        "peeled_commit": "0".repeat(40),
        "release_tag_verification": {
            "status": "not-observed",
            "signer_primary_fingerprint": null,
            "authority_path": null,
            "authority_blob_id": null,
            "verifier_result": null
        }
    })
}

fn verified_tag(version: &str) -> Value {
    json!({
        "name": version,
        "object_id": "0".repeat(40),
        "peeled_commit": "0".repeat(40),
        "release_tag_verification": {
            "status": "verified",
            "signer_primary_fingerprint": "1".repeat(40),
            "authority_path": "release-signers.toml",
            "authority_blob_id": "a".repeat(40),
            "verifier_result": "good signature; authorized primary fingerprint matched"
        }
    })
}

fn not_observed_archive(version: &str) -> Value {
    json!({
        "name": format!("prikk-v{version}.tar.gz"),
        "checksum_name": format!("prikk-v{version}.tar.gz.sha256"),
        "archive_sha256": null,
        "checksum_sha256": null,
        "archive_attached": false,
        "checksum_attached": false,
        "checksum_grammar": "not-observed",
        "archive_root": "not-observed"
    })
}

fn valid_archive(version: &str) -> Value {
    json!({
        "name": format!("prikk-v{version}.tar.gz"),
        "checksum_name": format!("prikk-v{version}.tar.gz.sha256"),
        "archive_sha256": "0".repeat(64),
        "checksum_sha256": "0".repeat(64),
        "archive_attached": true,
        "checksum_attached": true,
        "checksum_grammar": "valid",
        "archive_root": "valid"
    })
}

fn absent_release_page() -> Value {
    json!({"status": "absent"})
}

fn published_release_page() -> Value {
    json!({"status": "published"})
}

fn pending_pages() -> Value {
    json!({"status": "pending", "deployed_commit": null, "inapplicable_ruling": null})
}

fn deployed_pages() -> Value {
    json!({
        "status": "deployed",
        "deployed_commit": "0".repeat(40),
        "inapplicable_ruling": null
    })
}

fn unobserved_crate(name: &str, version: &str) -> CrateObservation {
    CrateObservation {
        name: name.to_string(),
        version: version.to_string(),
        exact_internal_requirements: true,
        staged_sha256: None,
        registry_checksum: None,
        fetched_sha256: None,
        published: false,
        registry_visible: false,
    }
}

fn matched_crate(name: &str, version: &str, sha256: &str) -> CrateObservation {
    CrateObservation {
        name: name.to_string(),
        version: version.to_string(),
        exact_internal_requirements: true,
        staged_sha256: Some(sha256.to_string()),
        registry_checksum: Some(sha256.to_string()),
        fetched_sha256: Some(sha256.to_string()),
        published: true,
        registry_visible: true,
    }
}

/// Every real workspace crate, `unobserved`. Matches today's actual eight members, so
/// `publish_levels` (run against the real repository root) resolves every name.
fn all_crates_unobserved(version: &str) -> Vec<CrateObservation> {
    [
        "prikk-error",
        "prikk-hash",
        "prikk-ffi",
        "prikk-crypto",
        "prikk-object",
        "prikk-replay",
        "prikk-store",
        "prikk",
    ]
    .into_iter()
    .map(|name| unobserved_crate(name, version))
    .collect()
}

fn all_crates_matched(version: &str, sha256: &str) -> Vec<CrateObservation> {
    [
        "prikk-error",
        "prikk-hash",
        "prikk-ffi",
        "prikk-crypto",
        "prikk-object",
        "prikk-replay",
        "prikk-store",
        "prikk",
    ]
    .into_iter()
    .map(|name| matched_crate(name, version, sha256))
    .collect()
}

fn pending_observations() -> Observations {
    Observations {
        version: "9.9.9".to_string(),
        tag: not_observed_tag("9.9.9"),
        archive: not_observed_archive("9.9.9"),
        crates: all_crates_unobserved("9.9.9"),
        release_page: absent_release_page(),
        pages: pending_pages(),
        governance: Value::Null,
        new_attempts: Vec::new(),
        superseded: false,
    }
}

fn complete_observations() -> Observations {
    Observations {
        version: "9.9.9".to_string(),
        tag: verified_tag("9.9.9"),
        archive: valid_archive("9.9.9"),
        crates: all_crates_matched("9.9.9", &"0".repeat(64)),
        release_page: published_release_page(),
        pages: deployed_pages(),
        governance: Value::Null,
        new_attempts: vec![json!({
            "time": "2026-09-06T00:00:00Z",
            "operation": "publish immutable release outputs",
            "result": "succeeded"
        })],
        superseded: false,
    }
}

/// Control 1: a `pending` document with nothing observed validates, carries three `null`
/// checksums per crate and `"not-observed"`, and does not claim `complete`.
///
/// **Seen to fail**: disabled the `nothing_attempted` branch in `derive_overall_status`
/// (`if false && nothing_attempted`). This fixture's own tag/archive/pages are not
/// complete-shaped either, so the status fell through to `"partial"` rather than `"pending"` --
/// still a genuine failure (`assert_eq!` against `"pending"` failed with `left: "partial"`),
/// confirming the branch is load-bearing rather than merely descriptive. Reverted.
#[test]
fn control1_pending_with_nothing_observed_validates_and_does_not_claim_complete() {
    let document = produce(&repo_root(), pending_observations(), None).unwrap();
    assert!(schema().is_valid(&document), "{document:#}");
    assert_eq!(document["overall_status"], "pending");
    for crate_row in document["crates"].as_array().unwrap() {
        assert!(crate_row["staged_sha256"].is_null());
        assert!(crate_row["registry_checksum"].is_null());
        assert!(crate_row["fetched_sha256"].is_null());
        assert_eq!(crate_row["checksum_equality"], "not-observed");
    }
}

/// Control 2: a fully-observed, all-equal document validates and claims `complete`.
///
/// **Seen to fail**: perturbed `checksum_equality`'s match arm to `staged == registry && registry
/// == fetched && false` -- every crate then read `"mismatch"` despite genuinely equal checksums,
/// and `overall_status` correctly followed it down to `"partial"` (`assert_eq!` against
/// `"complete"` failed with `left: "partial"`). Reverted.
#[test]
fn control2_fully_observed_all_equal_document_validates_and_claims_complete() {
    let document = produce(&repo_root(), complete_observations(), None).unwrap();
    assert!(schema().is_valid(&document), "{document:#}");
    assert_eq!(document["overall_status"], "complete");
    for crate_row in document["crates"].as_array().unwrap() {
        assert_eq!(crate_row["checksum_equality"], "match");
        assert_eq!(crate_row["published"], true);
        assert_eq!(crate_row["registry_visible"], true);
    }
}

/// Control 3: one mismatched checksum forces `partial`, not `complete` -- and the affected crate
/// reads `"mismatch"`. Both asserted, since a producer could get the crate row right and the
/// overall status wrong (or vice versa).
///
/// **Seen to fail**: perturbed `crates_complete` in `derive_overall_status` to drop the
/// `checksum_equality` check entirely (only `published`/`registry_visible`) -- the mismatched
/// crate's own row still correctly read `"mismatch"`, but `overall_status` was computed
/// `"complete"` over it regardless, exactly the "crate row right, overall status wrong" failure
/// mode this control names. Caught by `produce`'s own schema self-check before the test's own
/// assertions even ran (`"complete"` requires every crate's `checksum_equality == "match""`, which
/// the schema enforces structurally): `produce(..).unwrap()` panicked with `/crates/0/
/// checksum_equality: "match" was expected`. Reverted.
#[test]
fn control3_one_mismatched_checksum_forces_partial_not_complete() {
    let mut observations = complete_observations();
    observations.crates[0].fetched_sha256 = Some("f".repeat(64));
    let document = produce(&repo_root(), observations, None).unwrap();
    assert!(schema().is_valid(&document), "{document:#}");
    assert_eq!(document["overall_status"], "partial");
    assert_eq!(document["crates"][0]["checksum_equality"], "mismatch");
    for crate_row in document["crates"].as_array().unwrap().iter().skip(1) {
        assert_eq!(crate_row["checksum_equality"], "match");
    }
}

/// Control 4: `publish_level` is derived from the workspace dependency graph, not hardcoded --
/// proven against a synthetic workspace, where reordering/adding a member changes the derived
/// level. A test asserting only today's eight literal levels would pass against a hardcoded list
/// (`policy/evidence.rs`'s own `CRATE_ORDER` is exactly that hardcoded list, and it is stale: it
/// has seven entries, missing `prikk-ffi`, today's real eighth crate).
///
/// **Seen to fail**: perturbed `level_of` to always take its `own_deps.is_empty()` branch
/// (`if true { 1 } else { ... }`), simulating a derivation that ignores the graph. `b`'s expected
/// level (2, one hop above `a`) came out `1` instead (`assert_eq!` failed: `left: Some(1), right:
/// Some(2)`). Reverted.
#[test]
fn control4_publish_level_is_derived_from_the_dependency_graph_not_hardcoded() {
    let root = synthetic_workspace(&["a", "b", "c"], &[("b", &["a"]), ("c", &["b"])]);
    let levels = publish_levels(root.path()).unwrap();
    assert_eq!(levels.get("a"), Some(&1));
    assert_eq!(levels.get("b"), Some(&2));
    assert_eq!(levels.get("c"), Some(&3));

    // Add a fourth member depending on both `a` and `c` -- its level must follow the graph
    // (1 + max(level(a), level(c)) = 4), not a count of members or a fixed increment.
    let root_with_fourth = synthetic_workspace(
        &["a", "b", "c", "d"],
        &[("b", &["a"]), ("c", &["b"]), ("d", &["a", "c"])],
    );
    let levels_with_fourth = publish_levels(root_with_fourth.path()).unwrap();
    assert_eq!(levels_with_fourth.get("a"), Some(&1));
    assert_eq!(levels_with_fourth.get("b"), Some(&2));
    assert_eq!(levels_with_fourth.get("c"), Some(&3));
    assert_eq!(
        levels_with_fourth.get("d"),
        Some(&4),
        "a new member's level must follow the graph, not a hardcoded count: {levels_with_fourth:?}"
    );
}

/// Real, current, and against the actual repository -- confirms all eight of today's crates
/// resolve and reports what a hardcoded `CRATE_ORDER` would have to be updated to match (had this
/// module used one).
#[test]
fn control4_derives_levels_for_todays_eight_real_crates() {
    let levels = publish_levels(&repo_root()).unwrap();
    assert_eq!(levels.get("prikk-error"), Some(&1));
    assert_eq!(levels.get("prikk-hash"), Some(&1));
    assert_eq!(levels.get("prikk-ffi"), Some(&1));
    assert_eq!(levels.get("prikk-crypto"), Some(&2));
    assert_eq!(levels.get("prikk-object"), Some(&2));
    assert_eq!(levels.get("prikk-replay"), Some(&3));
    assert_eq!(levels.get("prikk-store"), Some(&4));
    assert_eq!(levels.get("prikk"), Some(&5));
}

/// Control 5: sequence and predecessor linkage. `001` has `prior_snapshot: null`; `002` names
/// `001` and its real SHA-256. Perturbing the predecessor hash (a caller's stale belief about what
/// `001` should hash to) is refused, not silently overridden by whatever bytes are actually on
/// disk.
///
/// **Seen to fail**: perturbed `load_prior`'s comparison to `if false && actual_sha256 !=
/// link.expected_sha256` (never refuses). The wrong-predecessor-hash case then produced `002`
/// successfully instead of refusing, and `assert!(rejected.is_err())` failed with the message
/// "a wrong predecessor hash must be refused, not silently corrected". Reverted.
#[test]
fn control5_sequence_and_predecessor_linkage() {
    let root = repo_root();
    let first = produce(&root, pending_observations(), None).unwrap();
    assert_eq!(first["sequence"], "001");
    assert!(first["prior_snapshot"].is_null());

    let directory = tempfile::tempdir().unwrap();
    let first_path = directory
        .path()
        .join("prikk-9.9.9-release-evidence-001.json");
    std::fs::write(&first_path, serde_json::to_vec_pretty(&first).unwrap()).unwrap();
    let real_sha256 = hex_sha256(&std::fs::read(&first_path).unwrap());

    let second = produce(
        &root,
        pending_observations(),
        Some(&PriorLink {
            path: first_path.clone(),
            expected_sha256: real_sha256.clone(),
        }),
    )
    .unwrap();
    assert_eq!(second["sequence"], "002");
    assert_eq!(
        second["prior_snapshot"]["name"],
        "prikk-9.9.9-release-evidence-001.json"
    );
    assert_eq!(second["prior_snapshot"]["sha256"], real_sha256);

    // Perturb: a caller-supplied predecessor hash that does not match the real file.
    let wrong_hash = "f".repeat(64);
    let rejected = produce(
        &root,
        pending_observations(),
        Some(&PriorLink {
            path: first_path,
            expected_sha256: wrong_hash,
        }),
    );
    assert!(
        rejected.is_err(),
        "a wrong predecessor hash must be refused, not silently corrected"
    );
}

/// Control 6: attempts are append-only. Building `002` from a state carrying `001`'s attempts
/// retains all of them, in order.
///
/// **Seen to fail**: perturbed the carry-forward step to start `attempts` from
/// `observations.new_attempts` alone (dropping `loaded.attempts`) -- `002` then carried only its
/// own new attempt, losing `001`'s. Restoring `loaded.attempts` as the starting point fixed it.
#[test]
fn control6_attempts_are_append_only() {
    let root = repo_root();
    let mut first_observations = pending_observations();
    first_observations.new_attempts = vec![json!({
        "time": "2026-09-01T00:00:00Z",
        "operation": "first attempt",
        "result": "failed"
    })];
    let first = produce(&root, first_observations, None).unwrap();
    assert_eq!(first["attempts"].as_array().unwrap().len(), 1);

    let directory = tempfile::tempdir().unwrap();
    let first_path = directory
        .path()
        .join("prikk-9.9.9-release-evidence-001.json");
    std::fs::write(&first_path, serde_json::to_vec_pretty(&first).unwrap()).unwrap();
    let real_sha256 = hex_sha256(&std::fs::read(&first_path).unwrap());

    let mut second_observations = pending_observations();
    second_observations.new_attempts = vec![json!({
        "time": "2026-09-02T00:00:00Z",
        "operation": "second attempt",
        "result": "succeeded"
    })];
    let second = produce(
        &root,
        second_observations,
        Some(&PriorLink {
            path: first_path,
            expected_sha256: real_sha256,
        }),
    )
    .unwrap();
    let attempts = second["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{attempts:#?}");
    assert_eq!(attempts[0]["sequence"], 1);
    assert_eq!(attempts[0]["operation"], "first attempt");
    assert_eq!(attempts[0]["result"], "failed");
    assert_eq!(attempts[1]["sequence"], 2);
    assert_eq!(attempts[1]["operation"], "second attempt");
    assert_eq!(attempts[1]["result"], "succeeded");
}

/// Control 7: the oracle accepts the output. Feed a produced document (both the pending and the
/// complete shapes) through the same schema-validation path the 73 existing oracle cases use --
/// `SchemaProfile`, compiled from the exact same `release/schemas/release-evidence-v1.schema.json`
/// `policy::evaluate` loads for the oracle's own manifest.
///
/// **Seen to fail, and a finding this control alone could not have caught**: perturbed
/// `checksum_equality` to unconditionally return `"match"`. This test still passed --
/// `$defs/crate` carries no unconditional constraint tying `checksum_equality` to the checksum
/// fields; the schema only forces that consistency inside the top-level `allOf`'s "if
/// `overall_status == "complete"`" block, so a `"pending"` or `"partial"` document can claim
/// `checksum_equality: "match"` over three `null` checksums and still be schema-valid. **Control
/// 7's own "the oracle accepts it" is therefore not sufficient, on its own, to guard handoff §3
/// item 1's named hazard** -- what actually caught this perturbation was control 1, which asserts
/// `checksum_equality == "not-observed"` directly rather than through schema validity, and whose
/// `nothing_attempted` derivation (reading that same field) cascaded into the wrong
/// `overall_status` (`"partial"` instead of `"pending"`). Reverted; recorded in the report rather
/// than left as a silent near-miss.
#[test]
fn control7_produced_documents_pass_the_same_schema_the_oracle_uses() {
    let schema = schema();
    let pending = produce(&repo_root(), pending_observations(), None).unwrap();
    assert!(schema.is_valid(&pending), "{:?}", schema.errors(&pending));
    let complete = produce(&repo_root(), complete_observations(), None).unwrap();
    assert!(schema.is_valid(&complete), "{:?}", schema.errors(&complete));
}

// RFC 141 §7b (`tighten-the-evidence-schema-handoff-v1.md`): the schema gap `control7` above found
// -- a `"pending"`/`"partial"`/`"superseded"` document could claim `checksum_equality: "match"` or
// `"mismatch"` over `null` checksums and still be schema-valid, because the shape constraint lived
// only inside the `overall_status == "complete"` branch -- is closed by a new `$defs/crate` `allOf`:
// when `checksum_equality` is `"match"` or `"mismatch"`, all three checksum fields must be present.
// The controls below exercise that new conditional directly, at every `overall_status`, never
// touching an oracle fixture (see the implementation report for why the oracle pack's own
// `pending_false_checksum_match` case is deliberately left unmodified pending an architect ruling).

/// §7b control 1: a dishonest document (`"match"` or `"mismatch"` claimed over three `null`
/// checksums) is rejected at every status the old constraint did *not* cover --
/// `"pending"`, `"partial"`, and `"superseded"` -- not only `"complete"`, since the whole point of
/// this change is that the old constraint was `complete`-only.
///
/// **Seen to fail**: removed the new `allOf` block from `$defs/crate` entirely (the pre-round
/// shape). Every one of the six documents (three statuses × two claims) then validated --
/// `assert!(!schema.is_valid(...))` failed on the first, printing the full six-hundred-plus-byte
/// document as `is_valid` returned `true`. Restored; `diff` against a saved copy of the schema file
/// empty afterward.
#[test]
fn tighten_control1_dishonest_checksum_equality_rejected_at_every_uncovered_status() {
    let schema = schema();
    for status in ["pending", "partial", "superseded"] {
        for claim in ["match", "mismatch"] {
            let mut document = produce(&repo_root(), pending_observations(), None).unwrap();
            document["overall_status"] = json!(status);
            document["crates"][0]["checksum_equality"] = json!(claim);
            assert!(
                !schema.is_valid(&document),
                "{claim:?} over null checksums must be rejected at overall_status={status:?}: \
                 {document:#}"
            );
        }
    }
}

/// §7b control 2: an honest document still validates in every status -- `"not-observed"` with
/// three `null`s (statuses where nothing has been attempted), and `"match"` with three present,
/// equal values (every status; a fully-observed crate row is not itself disqualifying at
/// `"pending"`/`"partial"`/`"superseded"`, only the *top-level* `"complete"` branch imposes
/// further requirements on the rest of the document).
///
/// **Seen to fail**: widened the new conditional's `if` to also match `"not-observed"`
/// (over-strict: presence required for every crate regardless of claim). Every `pending_observations`
/// crate row is `"not-observed"` with three `null`s, so `produce` itself could no longer emit a
/// document for that fixture at all -- `.unwrap()` on its own `Result` panicked with the same
/// "null is not of type \"string\"" shape as increment 1's own dishonest-input case, this time over
/// an honest one. Reverted.
#[test]
fn tighten_control2_honest_documents_still_validate_at_every_status() {
    let schema = schema();
    for status in ["pending", "partial", "superseded"] {
        let mut not_observed_document =
            produce(&repo_root(), pending_observations(), None).unwrap();
        not_observed_document["overall_status"] = json!(status);
        assert!(
            schema.is_valid(&not_observed_document),
            "not-observed must still validate at {status:?}: {not_observed_document:#}"
        );
    }
    for status in ["pending", "partial", "complete", "superseded"] {
        let mut matched_document = produce(&repo_root(), complete_observations(), None).unwrap();
        matched_document["overall_status"] = json!(status);
        assert!(
            schema.is_valid(&matched_document),
            "match with real, equal checksums must still validate at {status:?}: \
             {matched_document:#}"
        );
    }
}

/// §7b control 3: partial absence is caught too -- two checksums present and one `null` under
/// `"match"` must fail. A rule written against "all three null" (rather than "all three present")
/// would pass this and be wrong.
///
/// **Seen to fail**: perturbed the new conditional's `then` block to check only
/// `staged_sha256`/`registry_checksum` (dropping `fetched_sha256`). This exact case -- two present,
/// `fetched_sha256` left `null` -- then validated. Reverted.
#[test]
fn tighten_control3_partial_checksum_absence_under_match_is_rejected() {
    let schema = schema();
    let mut document = produce(&repo_root(), pending_observations(), None).unwrap();
    document["crates"][0]["checksum_equality"] = json!("match");
    document["crates"][0]["staged_sha256"] = json!("0".repeat(64));
    document["crates"][0]["registry_checksum"] = json!("0".repeat(64));
    // fetched_sha256 deliberately left null.
    assert!(
        !schema.is_valid(&document),
        "two present and one null under \"match\" must be rejected: {document:#}"
    );
}

// §7b control 5 (handoff §4): `produce`'s own self-validation now catches a dishonest document
// before returning it -- the exact defect increment 1's own `control7` found the *old* schema
// could not catch (see that test's own doc comment). Demonstrated as a genuine perturbation
// (`checksum_equality` forced to always return `"match"`, matching that same historical
// perturbation) rather than a permanent test, since `checksum_equality`'s own correct
// implementation can never organically produce a dishonest row for `produce` to catch: perturbing
// it to `fn checksum_equality(_: &CrateObservation) -> &'static str { "match" }` and calling
// `produce(&repo_root(), pending_observations(), None)` now returns `Err` naming every one of the
// eight crates' own `staged_sha256`/`registry_checksum`/`fetched_sha256` as `null is not of type
// "string"` -- where before this round's schema change the exact same perturbation returned `Ok`
// with a dishonest document (increment 1's own `control7` finding). Reverted; `git diff` against
// the committed source empty afterward. See the implementation report for the exact transcript.

/// Build a minimal, real, on-disk Cargo workspace with the given members and internal path
/// dependencies, and run `cargo generate-lockfile` against it so `publish_levels`'s own
/// `--locked --offline` `cargo metadata` invocation has a lockfile to read. Every member has zero
/// external dependencies, so this never touches the network.
fn synthetic_workspace(members: &[&str], edges: &[(&str, &[&str])]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let member_list = members
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        directory.path().join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{member_list}]\n"),
    )
    .unwrap();
    for member in members {
        let member_dir = directory.path().join(member);
        std::fs::create_dir_all(member_dir.join("src")).unwrap();
        std::fs::write(member_dir.join("src/lib.rs"), "").unwrap();
        let internal_deps = edges
            .iter()
            .find(|(name, _)| name == member)
            .map(|(_, deps)| *deps)
            .unwrap_or(&[]);
        let deps_section = if internal_deps.is_empty() {
            String::new()
        } else {
            let lines: Vec<String> = internal_deps
                .iter()
                .map(|dep| format!("{dep} = {{ path = \"../{dep}\" }}"))
                .collect();
            format!("\n[dependencies]\n{}\n", lines.join("\n"))
        };
        std::fs::write(
            member_dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{deps_section}"
            ),
        )
        .unwrap();
    }
    let status = std::process::Command::new(env!("CARGO"))
        .arg("generate-lockfile")
        .arg("--offline")
        .current_dir(directory.path())
        .status()
        .unwrap();
    assert!(status.success(), "cargo generate-lockfile failed");
    directory
}
