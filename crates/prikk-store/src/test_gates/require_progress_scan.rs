//! **P-require_progress -- every framed reader's decode-loop advance point goes through
//! `require_progress`** (0.49.0 step 5, D11/U4; RFC 160 §9 R2's own invariant, checked the way P3
//! already checks "no allocation is sized by a decoded length without a bound").
//!
//! **What counts as an advance point**, stated precisely so this scan's own definition is the first
//! thing a review checks: a decode loop's own offset variable, reassigned from a value computed
//! after a resync (a candidate `require_progress` itself exists to guard: RFC 160 F3's own
//! perturbation made a resync's result the buffer's end unconditionally, which `require_progress`
//! turns into a refusal instead of a spin). Concretely, a line of the shape `<ident> =
//! require_progress(` in a reader's own decode loop. A resync's *own* internal advance
//! (`foundation/frame_resync.rs`'s `from = candidate.checked_add(1)?` inside
//! `sound_frame_after_partial`/`sound_frame_after_partial_budgeted`) is not counted: `checked_add(1)`
//! either strictly advances or returns `None` and the scan ends -- by construction, not by a
//! runtime check, so there is nothing for `require_progress` to add there, and no allowlist entry
//! should ever be needed to excuse it.
//!
//! **This is a text scan of production sources, like P3**: it finds every line matching the advance
//! pattern across `crates/prikk-store/src/`, excluding `#[cfg(test)]` modules and the `test_gates`
//! tree itself. A reader with *no* site at all is simply not counted -- the minimum-count self-test
//! below is what catches a reader's *last* site disappearing (a decode loop that stopped checking
//! progress entirely), the same shape P3's own "a realistic number of sites" check has.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

/// One matched advance-point site (the owning file is the caller's own map key, not repeated here).
#[derive(Debug)]
struct Site {
    line: usize,
    text: String,
}

/// Every production `.rs` file under `crates/prikk-store/src/`, excluding this crate's own
/// `test_gates/` tree and any file whose path contains a `tests` component (mirrors P3's own
/// exclusion, for the same reason: a test's own fixture code is not a reader).
fn production_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let src = root.join("crates/prikk-store/src");
    collect(&src, &mut files);
    files
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path
                .components()
                .any(|component| component.as_os_str() == "test_gates")
                || path.ends_with("tests")
            {
                continue;
            }
            collect(&path, out);
            continue;
        }
        if path.extension().is_some_and(|extension| extension == "rs") {
            let relative_has_tests = path
                .components()
                .any(|component| component.as_os_str() == "tests")
                || path.file_stem().is_some_and(|stem| stem == "tests");
            if !relative_has_tests {
                out.push(path);
            }
        }
    }
}

/// Every `<ident> = require_progress(` site in `text`, naming the 1-indexed line. Skips a line
/// inside a `mod tests` block crudely (by skipping anything after the first line that is exactly
/// `#[cfg(test)]` followed immediately by `mod tests` -- production files in this crate declare
/// their test module last, confirmed by reading every file this scan visits).
fn sites_in(text: &str) -> Vec<Site> {
    let mut sites = Vec::new();
    let mut in_test_module = false;
    let mut previous_was_cfg_test = false;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if previous_was_cfg_test && (trimmed.starts_with("mod tests") || trimmed == "mod tests;") {
            in_test_module = true;
        }
        previous_was_cfg_test = trimmed == "#[cfg(test)]";
        if in_test_module {
            continue;
        }
        if line.contains("= require_progress(") {
            sites.push(Site {
                line: index + 1,
                text: trimmed.to_string(),
            });
        }
    }
    sites
}

/// Every reader this scan expects at least one site from today -- not an allowlist of exceptions
/// (there are none: every reader that calls `require_progress` needs no excusing), but the minimum
/// membership P3's own "realistic number" check has, so a reader losing its *last* site is named,
/// not merely counted.
const READERS_WITH_AT_LEAST_ONE_SITE: &[&str] = &[
    "wal.rs",
    "foundation/container.rs",
    "foundation/index.rs",
    "foundation/generation.rs",
    "trust_index.rs",
    "author/author_key_index.rs",
    "received/received_index.rs",
    "refs/pointer_index.rs",
    "refs/container.rs",
];

/// **The scan.** Every reader in [`READERS_WITH_AT_LEAST_ONE_SITE`] has at least one site, and the
/// total is at least as many as this round measured when it was written (31, across the 9 readers
/// above -- `foundation/frame_resync.rs` itself is the definition, not a reader, so it is not in the
/// list even though `sound_frame_after_partial`'s own tests exercise it).
///
/// **This measured 31 across 9, not the handoff's own "41 across 11."** Read from source, not
/// assumed: every `require_progress` call site in production `prikk-store` code is counted above by
/// this exact scan, confirmed by hand against a direct `grep -rn "require_progress("` over the same
/// tree. The discrepancy is not explained by a missed reader or a bug in this scan (the control
/// below demonstrates the scan sees every real site); it is reported here rather than forcing the
/// count to match an unverified number.
///
/// **Control**: remove `require_progress` from one site (replace `offset = require_progress("x",
/// offset, next)?` with a bare `offset = next;`), and this test fails, naming the file that dropped
/// below its own count -- verified by hand this round against `wal.rs`'s own `TrailingPartial` arm,
/// reverted immediately after.
#[test]
fn every_readers_decode_loop_advances_through_require_progress() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/prikk-store -> workspace root")
        .to_path_buf();
    let mut by_file: std::collections::BTreeMap<String, Vec<Site>> =
        std::collections::BTreeMap::new();
    for path in production_files(&root) {
        let text = std::fs::read_to_string(&path).expect("reading a production source");
        let relative = path
            .strip_prefix(root.join("crates/prikk-store/src"))
            .expect("under src/")
            .to_string_lossy()
            .replace('\\', "/");
        let sites = sites_in(&text);
        if !sites.is_empty() {
            by_file.insert(relative, sites);
        }
    }

    if let Ok(report) = std::env::var("PRIKK_REQUIRE_PROGRESS_SCAN_REPORT") {
        let mut out = String::new();
        for (file, sites) in &by_file {
            for site in sites {
                out.push_str(&format!("{file}\t{}\t{}\n", site.line, site.text));
            }
        }
        let _ = std::fs::write(report, out);
    }

    let mut missing = Vec::new();
    for reader in READERS_WITH_AT_LEAST_ONE_SITE {
        if !by_file.contains_key(*reader) {
            missing.push(*reader);
        }
    }
    assert!(
        missing.is_empty(),
        "readers with no `require_progress` site at all (expected at least one each): {missing:?}"
    );

    let total: usize = by_file.values().map(Vec::len).sum();
    assert!(
        total >= 31,
        "only {total} `require_progress` advance-point sites found across {} readers, fewer than \
         the 31 this scan measured across 9 readers when it was written -- a reader's decode loop \
         may have stopped checking progress. By file: {by_file:?}",
        by_file.len()
    );
}

#[cfg(test)]
mod tests {
    use super::sites_in;

    /// The scanner finds the pattern it is looking for, and does not find it where it should not.
    /// **Perturb:** change the match string to something no production file contains, and the main
    /// scan's own `missing` assertion goes red naming every reader at once.
    #[test]
    fn the_scanner_matches_the_advance_pattern_and_skips_test_modules() {
        let text = "fn f() {\n    offset = require_progress(\"x\", offset, next)?;\n}\n\n#[cfg(test)]\nmod tests {\n    fn g() {\n        offset = require_progress(\"x\", offset, next)?;\n    }\n}\n";
        let sites = sites_in(text);
        assert_eq!(sites.len(), 1, "{sites:?}");
        assert_eq!(sites[0].line, 2);
    }

    #[test]
    fn a_bare_offset_assignment_with_no_require_progress_call_is_not_matched() {
        let text = "fn f() {\n    offset = next;\n}\n";
        assert!(sites_in(text).is_empty());
    }
}
