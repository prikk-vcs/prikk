//! 0.50.0 step 1, A6 item 3 (019 §5.11): a removed (or never-written) witness reads *"acknowledged
//! commits: none recorded"* either way, by design (RFC 166 §9, never worse than 0.48.0) -- the two
//! are not told apart, on purpose. The line now says which rule is in force, so a reader does not
//! mistake silence for "nothing to check."

mod support;

#[test]
fn a_fresh_repository_names_the_pre_0_49_0_classification() {
    let repo = support::unique_repo("a6-item3-no-witness");
    support::init(&repo);
    let output = support::verify(&repo);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains(
            "acknowledged commits: none recorded (the classification of a session written \
             before 0.49.0 applies)"
        ),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
