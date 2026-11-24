#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::fs;
use std::path::Path;

use super::check;

const CI: &str = "\
jobs:
  stable:
    runs-on: ubuntu-latest
    steps:
      - name: Test
        run: cargo test --workspace --locked
  macos-mutation:
    runs-on: macos-latest
    steps:
      - name: Test
        run: cargo test --workspace --locked --no-fail-fast
      - name: Measure
        run: cargo test --workspace --locked -- some_measurement --ignored --nocapture
  windows-mutation:
    runs-on: windows-latest
    steps:
      - name: Test
        run: cargo test --workspace --locked --no-fail-fast
";

const ROOT_MANIFEST: &str = "[workspace]\nmembers = [\"crates/app\", \"crates/lib\"]\n\n[workspace.dependencies]\nlib = { path = \"crates/lib\" }\n";
const APP_MANIFEST: &str = "[package]\nname = \"app\"\n\n[dev-dependencies]\nlib = { workspace = true, features = [\"test-support\"] }\n";
const LIB_MANIFEST: &str = "[package]\nname = \"lib\"\n\n[features]\ntest-support = []\n";

const LIB_RS: &str = "\
pub fn production() {}

#[cfg(test)]
mod tests;

#[cfg(all(test, unix))]
mod unix_only;

#[cfg(all(test, feature = \"test-support\"))]
mod needs_feature;
";
const TESTS_RS: &str = "\
#[test]
fn builds_everywhere() {}

#[test]
#[cfg(not(windows))]
fn not_on_windows() {}

#[test]
#[ignore]
fn ignored_test() {}

#[test]
fn branches_at_run_time() {
    if cfg!(target_os = \"linux\") {
        return;
    }
}

#[test]
#[cfg(target_arch = \"x86_64\")]
fn only_x86_64() {}

#[test]
#[cfg(target_arch = \"aarch64\")]
fn only_aarch64() {}

fn helper_not_a_test() {}
";
const UNIX_ONLY_RS: &str = "#[test]\nfn only_unix() {}\n";
const NEEDS_FEATURE_RS: &str = "#[test]\nfn needs_the_feature() {}\n";

fn page(rows: &[&str]) -> String {
    let mut text = String::from(
        "# Page\n\nThe gate checks that each cited test exists and where it runs, not that it still asserts the property.\n\n<!-- assurance-table:begin -->\n| Crash point | Test | Kind | Runs on | Note |\n|---|---|---|---|---|\n",
    );
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    text.push_str("<!-- assurance-table:end -->\n");
    text
}

fn row(test: &str, file: &str, platforms: &str, note: &str) -> String {
    format!("| a crash point | `{test}` (`{file}`) | failpoint | {platforms} | {note} |")
}

fn fixture(rows: &[String]) -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    write(root, "Cargo.toml", ROOT_MANIFEST);
    write(root, "crates/app/Cargo.toml", APP_MANIFEST);
    write(root, "crates/app/src/main.rs", "fn main() {}\n");
    write(root, "crates/lib/Cargo.toml", LIB_MANIFEST);
    write(root, "crates/lib/src/lib.rs", LIB_RS);
    write(root, "crates/lib/src/tests.rs", TESTS_RS);
    write(root, "crates/lib/src/unix_only.rs", UNIX_ONLY_RS);
    write(root, "crates/lib/src/needs_feature.rs", NEEDS_FEATURE_RS);
    write(root, ".github/workflows/ci.yml", CI);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    write(
        root,
        "docs/src/reference/durability-recovery.md",
        &page(&rows),
    );
    temporary
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn errors_of(root: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    check(root, &mut errors).unwrap();
    errors
}

const TESTS: &str = "crates/lib/src/tests.rs";

fn good_rows() -> Vec<String> {
    vec![
        row("builds_everywhere", TESTS, "Linux, macOS, Windows", ""),
        row("not_on_windows", TESTS, "Linux, macOS", ""),
        row(
            "only_unix",
            "crates/lib/src/unix_only.rs",
            "Linux, macOS",
            "",
        ),
        row(
            "needs_the_feature",
            "crates/lib/src/needs_feature.rs",
            "Linux, macOS, Windows",
            "",
        ),
        row(
            "branches_at_run_time",
            TESTS,
            "Linux, macOS, Windows",
            "[os-varies]",
        ),
    ]
}

#[test]
fn a_correct_table_passes() {
    let fixture = fixture(&good_rows());
    assert_eq!(errors_of(fixture.path()), Vec::<String>::new());
}

/// Red control: rename a cited test.
#[test]
fn a_renamed_test_fails_naming_the_row() {
    let mut rows = good_rows();
    rows[1] = row("not_on_windows_renamed", TESTS, "Linux, macOS", "");
    let fixture = fixture(&rows);
    let errors = errors_of(fixture.path());
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("assurance-test-missing:row 2:not_on_windows_renamed"),
        "{errors:?}"
    );
}

/// Red control: type a wrong platform.
#[test]
fn a_wrong_platform_fails() {
    let mut rows = good_rows();
    rows[1] = row("not_on_windows", TESTS, "Linux, macOS, Windows", "");
    let errors = errors_of(fixture(&rows).path());
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("assurance-platform-mismatch:row 2:not_on_windows:row says Linux, macOS, Windows, derived Linux, macOS"), "{errors:?}");
}

/// Red control: a `cfg(unix)` put on a cited test whose row says Windows.
#[test]
fn a_cfg_added_to_a_windows_row_fails() {
    let fixture = fixture(&good_rows());
    write(
        fixture.path(),
        TESTS,
        &TESTS_RS.replace(
            "#[test]\nfn builds_everywhere",
            "#[test]\n#[cfg(unix)]\nfn builds_everywhere",
        ),
    );
    let errors = errors_of(fixture.path());
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .contains("builds_everywhere:row says Linux, macOS, Windows, derived Linux, macOS"),
        "{errors:?}"
    );
}

/// Red control: a run-time `cfg!(` with no marker, and a marker with no `cfg!(`.
#[test]
fn the_os_varies_marker_is_required_and_not_stale() {
    let mut rows = good_rows();
    rows[4] = row("branches_at_run_time", TESTS, "Linux, macOS, Windows", "");
    let errors = errors_of(fixture(&rows).path());
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("assurance-os-varies-unmarked:row 5:")),
        "{errors:?}"
    );

    let mut rows = good_rows();
    rows[0] = row(
        "builds_everywhere",
        TESTS,
        "Linux, macOS, Windows",
        "[os-varies]",
    );
    let errors = errors_of(fixture(&rows).path());
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("assurance-os-varies-stale:row 1:")),
        "{errors:?}"
    );
}

/// Red control: cite an `#[ignore]` test.
#[test]
fn an_ignored_test_cannot_be_cited() {
    let mut rows = good_rows();
    rows.push(row("ignored_test", TESTS, "Linux, macOS, Windows", ""));
    let errors = errors_of(fixture(&rows).path());
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("assurance-test-ignored:row 6:ignored_test")),
        "{errors:?}"
    );
}

#[test]
fn a_helper_that_is_not_a_test_cannot_be_cited() {
    let mut rows = good_rows();
    rows.push(row("helper_not_a_test", TESTS, "Linux, macOS, Windows", ""));
    let errors = errors_of(fixture(&rows).path());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("has no `#[test]`")),
        "{errors:?}"
    );
}

/// Red control: a filter on a suite step in a copy of `ci.yml`: the gate refuses to derive.
#[test]
fn a_filter_on_a_suite_step_refuses_the_derivation() {
    let fixture = fixture(&good_rows());
    write(
        fixture.path(),
        ".github/workflows/ci.yml",
        &CI.replace(
            "cargo test --workspace --locked --no-fail-fast\n      - name: Measure",
            "cargo test --workspace --locked --no-fail-fast -p lib\n      - name: Measure",
        ),
    );
    let errors = errors_of(fixture.path());
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("assurance-job-map:job `macos-mutation` runs"),
        "{errors:?}"
    );
}

#[test]
fn a_matrix_runs_on_or_a_conditional_job_refuses_the_derivation() {
    for (from, to, what) in [
        (
            "runs-on: windows-latest",
            "runs-on: ${{ matrix.os }}",
            "not a literal hosted runner",
        ),
        (
            "runs-on: windows-latest\n",
            "runs-on: windows-latest\n    if: github.ref == 'x'\n",
            "`if`",
        ),
        (
            "runs-on: windows-latest\n",
            "runs-on: windows-latest\n    continue-on-error: true\n",
            "continue-on-error",
        ),
    ] {
        let fixture = fixture(&good_rows());
        write(
            fixture.path(),
            ".github/workflows/ci.yml",
            &CI.replace(from, to),
        );
        let errors = errors_of(fixture.path());
        assert_eq!(errors.len(), 1, "{what}: {errors:?}");
        assert!(
            errors[0].starts_with("assurance-job-map:"),
            "{what}: {errors:?}"
        );
    }
}

#[test]
fn an_unknown_cfg_atom_fails_the_walk() {
    let fixture = fixture(&good_rows());
    write(
        fixture.path(),
        TESTS,
        &TESTS_RS.replace("#[cfg(not(windows))]", "#[cfg(not(debug_assertions))]"),
    );
    let errors = errors_of(fixture.path());
    assert!(
        errors
            .iter()
            .any(|error| error
                .contains("`cfg` atom `debug_assertions` is not one the gate evaluates")),
        "{errors:?}"
    );
}

#[test]
fn a_missing_table_or_statement_fails() {
    let fixture = fixture(&good_rows());
    write(
        fixture.path(),
        "docs/src/reference/durability-recovery.md",
        "# Page\n",
    );
    let errors = errors_of(fixture.path());
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("assurance-statement-missing:")),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.starts_with("assurance-table:")),
        "{errors:?}"
    );
}

/// A file no test root reaches is not built anywhere: a citation of a test in it fails.
#[test]
fn a_file_no_test_target_reaches_fails() {
    let fixture = fixture(&good_rows());
    write(
        fixture.path(),
        "crates/lib/src/orphan.rs",
        "#[test]\nfn orphaned() {}\n",
    );
    let mut rows = good_rows();
    rows.push(row("orphaned", "crates/lib/src/orphan.rs", "Linux", ""));
    write(
        fixture.path(),
        "docs/src/reference/durability-recovery.md",
        &page(&rows.iter().map(String::as_str).collect::<Vec<_>>()),
    );
    let errors = errors_of(fixture.path());
    assert!(
        errors
            .iter()
            .any(|error| error.contains("is not part of any test target")),
        "{errors:?}"
    );
}

// ---- 0.59.0 K5: the two assumptions behind a `(x86_64)` label are checks ----------------------------------------------------------------------------

/// A row labelled `(x86_64)` cites a test whose `cfg` names `target_arch = "x86_64"`: it passes, and it is the Linux and Windows runners that are x86_64.
#[test]
fn an_x86_64_label_on_a_test_gated_on_it_passes() {
    let rows = vec![row(
        "only_x86_64",
        TESTS,
        "Linux (x86_64), Windows (x86_64)",
        "",
    )];
    assert_eq!(errors_of(fixture(&rows).path()), Vec::<String>::new());
}

/// Red control (the label): the same label on a test with no architecture gate, and on one gated on the other architecture.
#[test]
fn an_x86_64_label_on_a_test_the_cfg_does_not_gate_on_it_fails() {
    let rows = vec![row(
        "builds_everywhere",
        TESTS,
        "Linux (x86_64), macOS, Windows",
        "",
    )];
    let errors = errors_of(fixture(&rows).path());
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with(
            "assurance-arch-label:row 1:builds_everywhere:the row says `Linux (x86_64)`"
        ),
        "{errors:?}"
    );
    // Gated on aarch64, so built only where the runner is arm64 (macOS): the label is false there too.
    let rows = vec![row("only_aarch64", TESTS, "macOS (x86_64)", "")];
    let errors = errors_of(fixture(&rows).path());
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0]
            .starts_with("assurance-arch-label:row 1:only_aarch64:the row says `macOS (x86_64)`"),
        "{errors:?}"
    );
}

/// Red control (the runners): the architecture of a runner is a fact about its label. A suite job on any other label (an arm Linux runner, an Intel
/// macOS runner), or a missing one of the three, fails the gate naming the labels it found.
#[test]
fn a_suite_job_on_another_runner_label_fails_naming_it() {
    let rows = vec![row(
        "only_x86_64",
        TESTS,
        "Linux (x86_64), Windows (x86_64)",
        "",
    )];
    for (from, to) in [
        ("runs-on: ubuntu-latest", "runs-on: ubuntu-24.04-arm"),
        ("runs-on: macos-latest", "runs-on: macos-13"),
        ("runs-on: windows-latest", "runs-on: windows-11-arm"),
    ] {
        let fixture = fixture(&rows);
        write(
            fixture.path(),
            ".github/workflows/ci.yml",
            &CI.replace(from, to),
        );
        let errors = errors_of(fixture.path());
        assert_eq!(errors.len(), 1, "{to}: {errors:?}");
        assert!(
            errors[0].starts_with("assurance-job-map:")
                && errors[0].contains(to.trim_start_matches("runs-on: "))
                && errors[0].contains(
                    "ubuntu-latest (x86_64), windows-latest (x86_64), macos-latest (aarch64)"
                ),
            "{to}: {errors:?}"
        );
    }
    // A dropped suite job leaves two of the three.
    let fixture = fixture(&rows);
    let without_macos = CI.replace("runs-on: macos-latest", "runs-on: windows-latest");
    write(fixture.path(), ".github/workflows/ci.yml", &without_macos);
    let errors = errors_of(fixture.path());
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("the suite jobs run on ubuntu-latest, windows-latest; the gate's `target_arch` assumption is for exactly"), "{errors:?}");
}
