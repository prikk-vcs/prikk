//! **P1 -- the whole-read guard** (RFC 160 §3.1; compiled in test builds only, so a shipped binary contains none of it).
//!
//! Under `cfg(test)` the anchored reader **fails the test that reads a store-growing file whole**, unless the read happens inside a
//! declared scope ([`declare`], one row of [`SCOPES`]). Store-growing files are the ones whose size follows what the repository has
//! stored: the object containers, the object index and the ref log containers ([`store_growing_kind`]). The whole store test suite is
//! the detector: a production path that starts reading one of them whole fails whichever test reaches it, naming the path and the
//! caller. (RFC 102's append-length round found three such reads that had run for six weeks with nothing to say so.)
//!
//! **Adding a scope is a decision, not a fix for a red test.** A row needs its reason and the RFC or ROADMAP reference that bounds
//! the cost it declares; a whole read with neither is a defect to name, and the reading is to be replaced by a ranged read
//! (`read_file_range_if_exists`). A row whose cost follows the store and is not owned by a ruling is `OpenFinding`, and the round's
//! report names it: the table is where the debt is visible.
//!
//! **Report-only mode** (the inventory this guard began with): when `PRIKK_WHOLE_READ_REPORT` names a file, a whole read of a
//! store-growing file is *recorded there* (kind, scope or `-`, the nearest `prikk_store` callers, the running test) instead of failing.

use std::cell::RefCell;
use std::path::Path;

/// Whether a declared scope is a cost the store means to have, or one this table records as **an open finding** (a whole read on a
/// path whose cost follows the store, declared so the suite can run, with a ruling requested in the round's report).
pub(crate) enum ScopeStatus {
    /// The whole read is the operation's definition, or a cost an RFC / ROADMAP row already owns.
    Intentional,
    /// A per-operation whole read that follows the store's size. **Declared, not blessed**: the report names it for a ruling, and the
    /// row goes away in the round that fixes it.
    OpenFinding,
}

/// One declared scope: a production path that is allowed to read a store-growing file whole, why, and what bounds the cost.
pub(crate) struct DeclaredScope {
    pub(crate) id: &'static str,
    pub(crate) status: ScopeStatus,
    pub(crate) reason: &'static str,
    pub(crate) reference: &'static str,
}

/// **The table of declared scopes.** Every row is a whole read of a store-growing file that production code does today.
pub(crate) const SCOPES: &[DeclaredScope] = &[
    DeclaredScope {
        id: "index-whole-decode",
        status: ScopeStatus::Intentional,
        reason: "`replay_index_with_extent` decodes the whole object index: once per session for `IndexSnapshot::open`, once per call                  for `FileObjectStore`'s lookups (`lookup_object_location`, per read / has / write) and for every read command's                  snapshot. The per-write refresh after a session's own write is a tail read (`replay_index_tail_with_extent`) and is                  not this scope. The linear lookup is what the index's own ROADMAP row owns",
        reference: "RFC 111 §6.1; ROADMAP AUD-01 (the object index's linear lookup); RFC 160 §3.1 and §6 (out of scope there)",
    },
    DeclaredScope {
        id: "verify-scan",
        status: ScopeStatus::Intentional,
        reason: "`verify` reads every object container and every ref log container to check every record; that is what verifying is,                  and its cost is the store's by definition",
        reference: "RFC 102 Stage 5; RFC 160 §3.1",
    },
    DeclaredScope {
        id: "index-rebuild",
        status: ScopeStatus::Intentional,
        reason: "the index rebuild behind `doctor --repair-index` re-derives the object index from every container, and the repair reads the index as it stands to compare",
        reference: "RFC 102 Stage 3 (repair-index); RFC 160 §3.1",
    },
    DeclaredScope {
        id: "type-enumeration",
        status: ScopeStatus::OpenFinding,
        reason: "`accepted_but_unsealed_patch_ids`, `enumerate_stored_claims` and `received_tag_ids` list every object of one type by                  reading that type's whole container: there is no by-type index, so listing costs the type's container. The listing                  commands are the operation's definition; but `seal_from_accepted_claim` and `refuse_if_order_ambiguous` call two of                  them on the way to sealing a claim, so a per-claim cost follows the patch and claim containers",
        reference: "RFC 160 report v1 §F2 (ruling requested: an index by type, or leave the listing commands and re-scope the seal-path calls)",
    },
    DeclaredScope {
        id: "ref-log-replay",
        status: ScopeStatus::OpenFinding,
        reason: "`replay_ref_subsequence`, `incomplete_tail_matches` and `truncate_incomplete_tail` read the whole ref log container                  (every ref's history) to answer one ref's question: a ref publication does it up to three times                  (`append_ref_container_record`, `classify_state`, `ensure_agreement`), so the cost of updating one ref follows                  the number of ref updates the repository has ever recorded",
        reference: "RFC 160 report v1 §F1 (ruling requested); RFC 102 Stage 6 (ref log containers, never compacted, DC-38/DC-69)",
    },
];

/// Which family of store-growing file `relative` (relative to the repository's `.prikk/` directory) is in, if it is in one.
pub(crate) fn store_growing_kind(relative: &Path) -> Option<&'static str> {
    let text = relative.to_str()?.replace('\\', "/");
    if text == "containers/index.container" {
        return Some("object index");
    }
    if let Some(rest) = text.strip_prefix("containers/") {
        // `containers/<type>/<slot>.container` -- an object container. (`generations.log` and the like are not containers.)
        if rest.ends_with(".container") && rest.matches('/').count() == 1 {
            return Some("object container");
        }
    }
    if let Some(rest) = text.strip_prefix("refs/containers/") {
        if rest.starts_with("log-") && rest.ends_with(".container") {
            return Some("ref log container");
        }
    }
    None
}

thread_local! {
    static OPEN_SCOPES: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// A declared scope, open until dropped. `id` must be a row of [`SCOPES`] (a typo is a panic, not a silent no-op).
pub(crate) struct ScopeGuard(());

/// Open the declared scope `id` on this thread.
pub(crate) fn declare(id: &'static str) -> ScopeGuard {
    assert!(
        SCOPES.iter().any(|row| row.id == id),
        "whole-read scope `{id}` is not a row of `whole_read_guard::SCOPES`"
    );
    OPEN_SCOPES.with(|open| open.borrow_mut().push(id));
    ScopeGuard(())
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        OPEN_SCOPES.with(|open| {
            open.borrow_mut().pop();
        });
    }
}

/// The scope this thread is in, if any (the innermost).
fn current_scope() -> Option<&'static str> {
    OPEN_SCOPES.with(|open| open.borrow().last().copied())
}

/// Called by the anchored whole read before it reads: fail the running test if `relative` is store-growing and no declared scope
/// covers the read (or, in report-only mode, record it).
pub(super) fn check_whole_read(relative: &Path) {
    // Measurement only (RFC 160 G1: what the guard costs the suite): `PRIKK_WHOLE_READ_GUARD=off` skips the check, so one build can be
    // timed with and without it. Nothing else sets it; a suite run with it set proves nothing about whole reads.
    if std::env::var_os("PRIKK_WHOLE_READ_GUARD").is_some_and(|value| value == "off") {
        return;
    }
    let Some(kind) = store_growing_kind(relative) else {
        return;
    };
    let scope = current_scope();
    if let Ok(report) = std::env::var("PRIKK_WHOLE_READ_REPORT") {
        record(&report, kind, scope, relative);
        return;
    }
    if scope.is_some() {
        return;
    }
    panic!(
        "whole read of a store-growing file ({kind}): `{}`, outside every declared scope. A read that follows the size of the store \
         is what RFC 102's append-length round removed three of; use a ranged read, or -- if the whole read is the point -- add a \
         row (with its reason and reference) to `whole_read_guard::SCOPES` and declare it at the call site. Callers: {}",
        relative.display(),
        callers().join(" <- ")
    );
}

/// The nearest `prikk_store` frames above the reader (function names only).
fn callers() -> Vec<String> {
    let trace = std::backtrace::Backtrace::force_capture().to_string();
    let mut found = Vec::new();
    for line in trace.lines() {
        let Some((_, symbol)) = line.trim().split_once(": ") else {
            continue;
        };
        if !symbol.starts_with("prikk_store::") || symbol.contains("fsutil::anchored") {
            continue;
        }
        // Drop the trailing `::h<hash>` and the closure marker so one function is one name.
        let name = symbol
            .rsplit_once("::h")
            .map_or(symbol, |(name, _)| name)
            .to_string();
        if name.contains("::tests::") || name.contains("test_gates") {
            break;
        }
        found.push(name);
        if found.len() == 3 {
            break;
        }
    }
    found
}

fn record(report: &str, kind: &str, scope: Option<&str>, relative: &Path) {
    use std::io::Write as _;
    let test = std::thread::current()
        .name()
        .unwrap_or("<unnamed thread>")
        .to_string();
    let line = format!(
        "{kind}\t{}\t{}\t{}\t{test}\n",
        scope.unwrap_or("-"),
        relative.display(),
        callers().join(" <- ")
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(report)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::path::Path;

    use super::{SCOPES, ScopeStatus, declare, store_growing_kind};

    /// The classifier names the three families and nothing else -- in particular not the compacting containers, whose size follows
    /// refs and trust records rather than objects.
    /// **Perturb:** return `Some` for every `.container`: the second half goes red.
    #[test]
    fn the_guard_classifies_the_three_growing_families_and_only_those() {
        assert_eq!(
            store_growing_kind(Path::new("containers/blob/a.container")),
            Some("object container")
        );
        assert_eq!(
            store_growing_kind(Path::new("containers/patch/a.container")),
            Some("object container")
        );
        assert_eq!(
            store_growing_kind(Path::new("containers/index.container")),
            Some("object index")
        );
        assert_eq!(
            store_growing_kind(Path::new("refs/containers/log-a.container")),
            Some("ref log container")
        );
        for other in [
            "containers/generations.log",
            "refs/containers/pointer-index-a.container",
            "refs/containers/received-index-a.container",
            "trust/policy-a.container",
            "HEAD",
            "wal/active.wal",
        ] {
            assert_eq!(store_growing_kind(Path::new(other)), None, "{other}");
        }
    }

    /// The table is well-formed: every row has an id, a reason and a reference, and ids are unique. **A row without a reason is a
    /// scope declared to make a suite pass**, which the guard exists to prevent.
    #[test]
    fn every_declared_scope_has_a_reason_and_a_reference_and_ids_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for row in SCOPES {
            assert!(!row.id.is_empty() && seen.insert(row.id), "id {}", row.id);
            assert!(row.reason.len() > 40, "{}: state the reason", row.id);
            assert!(
                row.reference.contains("RFC") || row.reference.contains("ROADMAP"),
                "{}: name the RFC or ROADMAP reference",
                row.id
            );
            if matches!(row.status, ScopeStatus::OpenFinding) {
                assert!(
                    row.reference.contains("report"),
                    "{}: an open finding names the report that asks for its ruling",
                    row.id
                );
            }
        }
    }

    /// **The guard fires.** A whole read of an object container, of the object index and of a ref log container, outside every
    /// declared scope, fails with the message that names the file and how to proceed; inside a declared scope it does not; a file
    /// that does not grow with the store (`FORMAT`) is never refused; and a ranged read of a container is not a whole read.
    /// **Perturb:** delete the `check_whole_read` call in `read_file_if_exists`: the first assertion goes red (and so does every
    /// production path a test reaches, which is the point).
    #[test]
    fn a_whole_read_of_a_store_growing_file_outside_a_declared_scope_fails() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        use crate::foundation::fsutil::{read_file_if_exists, read_file_range_if_exists};
        use crate::foundation::layout::RepositoryLayout;

        if std::env::var("PRIKK_WHOLE_READ_REPORT").is_ok()
            || std::env::var_os("PRIKK_WHOLE_READ_GUARD").is_some()
        {
            return; // report-only and measurement modes do not fail; this control is about the failing mode
        }
        let root = crate::test_gates::test_support::unique_temp_dir("whole-read-guard-fires");
        let layout = RepositoryLayout::init(root.clone()).expect("init");
        let mutation = layout.repository_mutation_root();
        for relative in [
            "containers/blob/a.container",
            "containers/index.container",
            "refs/containers/log-a.container",
        ] {
            let path = Path::new(relative);
            let refused = catch_unwind(AssertUnwindSafe(|| read_file_if_exists(mutation, path)));
            let payload = refused.expect_err(relative);
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_default();
            assert!(
                message.contains("outside every declared scope") && message.contains(relative),
                "{relative}: {message}"
            );
            {
                let _scope = declare("verify-scan");
                read_file_if_exists(mutation, path).expect("declared scope reads");
            }
            read_file_range_if_exists(mutation, path, 0, usize::MAX)
                .expect("a ranged read is not a whole read");
        }
        read_file_if_exists(mutation, Path::new("FORMAT"))
            .expect("FORMAT does not grow with the store");
        let _ = std::fs::remove_dir_all(root);
    }

    /// An undeclared scope id is a panic, not a silent pass.
    #[test]
    #[should_panic(expected = "is not a row of")]
    fn declaring_a_scope_that_is_not_in_the_table_panics() {
        let _scope = declare("not-a-row");
    }
}
