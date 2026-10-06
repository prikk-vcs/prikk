//! The one classifier for "is this source file test code?", used by every scan in this crate that skips test files.
//!
//! It works on path **components**, never on path text. A Windows walk spells a separator `\`, so a `contains("/tests/")`
//! over the text skips nothing there, and the scan that relied on it reported test code as production (0.49.0 step 5,
//! round 3 addendum). Callers pass a path relative to the crate's `src/`.

use std::path::Path;

pub(crate) fn is_test_source(relative: &Path) -> bool {
    let name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name.ends_with("tests.rs") || name.ends_with("_test.rs") || name.contains("_tests") {
        return true;
    }
    relative.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        part == "tests"
            || part == "test_gates"
            || part == "test_gates.rs"
            || part.contains("caller_tests")
            || part.contains("test_support")
    })
}

#[cfg(test)]
mod tests {
    use super::is_test_source;
    use std::path::PathBuf;

    fn path(parts: &[&str]) -> PathBuf {
        parts.iter().collect()
    }

    /// The component reading, on paths built from components (what a Windows walk yields). Linux and macOS cannot
    /// render a backslash path, so this proves the reading and the Windows mutation suite proves the platform.
    /// **Perturb:** classify by `relative.to_string_lossy().contains("/tests/")`: this stays green on Linux (its separator
    /// is `/`), which is the defect's whole shape; the scans that use `is_test_source` are what the Windows job runs.
    #[test]
    fn test_sources_are_classified_by_component_not_by_spelling() {
        assert!(is_test_source(&path(&["snapshot", "tests", "writer.rs"])));
        assert!(is_test_source(&path(&[
            "block_state",
            "tests",
            "deep",
            "x.rs"
        ])));
        assert!(is_test_source(&path(&["merge", "execute", "tests.rs"])));
        assert!(is_test_source(&path(&["seal_from_accepted", "tests.rs"])));
        assert!(is_test_source(&path(&["test_gates", "test_support.rs"])));
        assert!(is_test_source(&path(&["test_gates.rs"])));
        // Rows for every marker the witness scan used (`caller_tests*`, `test_support*`), so its classification is not
        // narrowed by the move: as a directory, and as a file name.
        assert!(is_test_source(&path(&[
            "commit_boundary",
            "caller_tests",
            "x.rs"
        ])));
        assert!(is_test_source(&path(&["caller_tests_helper.rs"])));
        assert!(is_test_source(&path(&["foundation", "test_support.rs"])));
        assert!(is_test_source(&path(&["test_support_gating", "x.rs"])));
        assert!(is_test_source(&path(&[
            "wal",
            "ref_name_once_per_session_tests.rs"
        ])));
        assert!(!is_test_source(&path(&["block_state.rs"])));
        assert!(!is_test_source(&path(&[
            "block_state",
            "anchored_parent.rs"
        ])));
        assert!(!is_test_source(&path(&["merge", "execute.rs"])));
        // A component that merely contains the word is not the directory.
        assert!(!is_test_source(&path(&["contests", "x.rs"])));
        assert!(!is_test_source(&path(&["tests_helper.rs"])));
    }
}
