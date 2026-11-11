//! 0.57.0 step 3, part S5d: after `received remove`, the unreached **content** (the patch) is the count line, while the block and
//! the RefState it leaves keep a warning each, because their publication trust was not checked. The JSON still lists all three.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use super::u4_unreached_outside_version_rule::{
    doctor, received_non_ascii_chain, remove_received, verify,
};
use crate::support;

fn text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn the_patch_is_counted_and_the_block_and_ref_state_keep_their_warnings() {
    let (repo, block, patch) = received_non_ascii_chain("s5d-received-remove");
    remove_received(&repo);

    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(0), "{said}");
    assert!(said.contains("unreached objects: 3"), "{said}");
    assert!(
        said.contains(&format!(
            "warning: block {block} is reached by nothing in this repository -- its publication trust was not checked"
        )),
        "{said}"
    );
    assert_eq!(
        said.lines()
            .filter(|line| line.starts_with("warning: ref-state ")
                && line.contains("is reached by nothing"))
            .count(),
        1,
        "the RefState keeps its warning\n{said}"
    );
    assert!(
        !said.contains(&format!("patch {patch} is reached by nothing")),
        "{said}"
    );
    assert_eq!(
        said.lines()
            .filter(
                |line| line.starts_with("unreached content: 1 object(s) (1 patch(es), 0 blob(s))")
            )
            .count(),
        1,
        "{said}"
    );

    let doctor_output = doctor(&repo);
    let doctor_said = text(&doctor_output);
    assert_eq!(doctor_output.status.code(), Some(0), "{doctor_said}");
    assert_eq!(
        doctor_said
            .matches("warning [PRIKK-DOCTOR-UNREACHED-OBJECT]")
            .count(),
        2,
        "block and RefState\n{doctor_said}"
    );
    assert_eq!(
        doctor_said
            .matches("info [PRIKK-DOCTOR-UNREACHED-CONTENT]")
            .count(),
        1,
        "{doctor_said}"
    );

    // A precision change must not report less: the JSON lists every object, each with its type.
    let json = support::prikk(&repo)
        .args(["verify", "--format", "json"])
        .output()
        .unwrap();
    let value = support::json::parse(&String::from_utf8_lossy(&json.stdout));
    let mut types: Vec<String> = value
        .get("unreached_objects")
        .as_array()
        .iter()
        .map(|object| object.get("object_type").as_str().to_string())
        .collect();
    types.sort();
    assert_eq!(types, ["block", "patch", "ref-state"]);
    let _ = std::fs::remove_dir_all(&repo);
}
