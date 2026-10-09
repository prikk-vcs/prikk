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
        id: "recovery-log-identity",
        status: ScopeStatus::OpenFinding,
        reason: "a repair and a restore record or check the identity of the meaning files an entry names (RFC 168 §3.2, condition 3). \
                 Two of them are store-growing: the ref log container (a pointer-index entry's meaning) and the pointer index container \
                 (the ref log's). The identity is a whole-file SHA-256, so the cost follows the size of the file it names. Repairs are \
                 rare and restores are rarer, but the cost is the store's",
        reference: "RFC 168 §3.1-§3.2 (the meaning-file table); the round's report names the bounded identity as the open question",
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
    DeclaredScope {
        id: "pointer-index-rebuild",
        status: ScopeStatus::Intentional,
        reason: "`prikk doctor --rebuild-pointer-index` re-derives the whole ref-pointer index from every record in the ref log                  container, once per run (`decode_ref_log_for_rebuild`); re-deriving the pointer index from the ref log is                  what rebuilding means, the same reasoning `index-rebuild` already has for the object index",
        reference: "RFC 165 R5",
    },
    // 0.49.0 step 5, D11/P1: the four rows below extend this guard to the other RFC 167 M5 readers
    // (the WAL, pointer index, received index, trust policy). Each one's own replay function reads
    // its file whole, by the same reasoning `index-whole-decode` already has for the object index:
    // there is no "tail-only" form of "decode every record this file holds", so a whole read is the
    // operation's definition, not a cost that follows a caller's own choice. Each is declared exactly
    // once, inside the one function every caller of that family funnels through (`Wal::read_bytes`,
    // `replay_pointer_index`, `replay_received_index`, `replay_trust_policy`), so no caller elsewhere
    // needs to know this guard exists.
    DeclaredScope {
        id: "wal-replay",
        status: ScopeStatus::Intentional,
        reason: "`Wal::replay` (via `Wal::read_bytes`) decodes the whole active WAL: replaying a log means reading every record it                  holds, the same reasoning `index-whole-decode` already has for the object index",
        reference: "RFC 167 M5 (0.49.0 step 4); RFC 160 §3.1",
    },
    DeclaredScope {
        id: "pointer-index-replay",
        status: ScopeStatus::Intentional,
        reason: "`replay_pointer_index` decodes the whole live pointer-index slot: the same reasoning `index-whole-decode` has for                  the object index, for the ref pointer index's own container",
        reference: "RFC 167 M5 (0.49.0 step 4); RFC 160 §3.1",
    },
    DeclaredScope {
        id: "received-index-replay",
        status: ScopeStatus::Intentional,
        reason: "`replay_received_index` decodes the whole live received-index slot: the same reasoning `index-whole-decode` has                  for the object index, for the received-ref index's own container",
        reference: "RFC 167 M5 (0.49.0 step 4); RFC 160 §3.1",
    },
    DeclaredScope {
        id: "trust-policy-replay",
        status: ScopeStatus::Intentional,
        reason: "`replay_trust_policy` decodes the whole live trust-policy slot: the same reasoning `index-whole-decode` has for                  the object index, for the trust-policy container",
        reference: "RFC 167 M5 (0.49.0 step 4); RFC 160 §3.1",
    },
    DeclaredScope {
        id: "generation-resolver-deduction",
        status: ScopeStatus::Intentional,
        reason: "`resolve_or_deduce` reads both of a compacting container's slots, whole, only in the rare state where the                  generation log names no slot and slot B holds data -- content is the only way left to decide between them",
        reference: "RFC 102 Stage 6 Step 2 (0.50.0 step 1 Part E2)",
    },
    DeclaredScope {
        id: "pointer-index-rebuild-recovery-save",
        status: ScopeStatus::Intentional,
        reason: "`run_pointer_index_rebuild` reads the whole live slot once, only on a real run that writes, to save it to the                  recovery log before flipping away from it -- the saved copy is the way back, so it must be the whole slot, not a range",
        reference: "RFC 165 R5 (0.50.0 step 1 Part F)",
    },
    DeclaredScope {
        id: "compaction-over-deduced-live-slot-recovery-save",
        status: ScopeStatus::Intentional,
        reason: "a compaction whose own live slot was deduced, not read off a recorded generation entry, reads the whole target                  slot and generation log once, only on a real run, to save both to the recovery log before overwriting them -- the                  saved copy is the way back, so it must be the whole file, not a range",
        reference: "RFC 165 Q2 (0.50.0)",
    },
];

/// Which family of store-growing file `relative` (relative to the repository's `.prikk/` directory) is in, if it is in one.
///
/// 0.49.0 step 5, D11/P1: four more families, each a RFC 167 M5 reader this guard did not see before
/// -- the pointer index, the received index, trust policy, and the WAL. Each one's own cost already
/// follows the store the same way the original three do (RFC 167's own six quadratic readers minus
/// the two -- object containers, the ref log -- this guard already covered).
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
        if rest.starts_with("pointer-index-") && rest.ends_with(".container") {
            return Some("pointer index");
        }
        if rest.starts_with("received-index-") && rest.ends_with(".container") {
            return Some("received index");
        }
    }
    if let Some(rest) = text.strip_prefix("trust/") {
        if rest.starts_with("policy-") && rest.ends_with(".container") {
            return Some("trust policy");
        }
    }
    if let Some(rest) = text.strip_prefix("active/") {
        if rest.ends_with("/queue.wal") {
            return Some("WAL");
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

    /// The classifier names the seven families and nothing else -- in particular not the compacting
    /// containers that are not one of the seven (trust keys, author keys, the three generation
    /// logs), whose size follows refs and trust records rather than objects, and not the ref-name/
    /// witness files that sit beside the WAL in the same `active/<name>/` directory.
    /// 0.49.0 step 5, D11/P1: four more families added to the original three -- the pointer index,
    /// the received index, trust policy, and the WAL -- the other RFC 167 M5 readers.
    /// **Perturb:** return `Some` for every `.container`: the non-families half goes red.
    #[test]
    fn the_guard_classifies_the_seven_growing_families_and_only_those() {
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
        assert_eq!(
            store_growing_kind(Path::new("refs/containers/pointer-index-a.container")),
            Some("pointer index")
        );
        assert_eq!(
            store_growing_kind(Path::new("refs/containers/received-index-b.container")),
            Some("received index")
        );
        assert_eq!(
            store_growing_kind(Path::new("trust/policy-a.container")),
            Some("trust policy")
        );
        assert_eq!(
            store_growing_kind(Path::new("active/default/queue.wal")),
            Some("WAL")
        );
        assert_eq!(
            store_growing_kind(Path::new("active/some-other-session/queue.wal")),
            Some("WAL")
        );
        for other in [
            "containers/generations.log",
            "refs/containers/pointer-index-generation.log",
            "refs/containers/received-index-generation.log",
            "trust/policy-generation.log",
            "trust/keys.container",
            "trust/author-keys.container",
            "active/default/ref-name",
            "active/default/witness",
            "HEAD",
            "FORMAT",
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
        // 0.49.0 step 5, D11/P1: the four new families, each paired with one of its own declared
        // scopes (any one of them, when a family has more than one caller-site scope) -- the same
        // "fails outside, succeeds inside, a ranged read is never a whole one" proof the original
        // three already had.
        for (relative, scope_id) in [
            ("containers/blob/a.container", "verify-scan"),
            ("containers/index.container", "verify-scan"),
            ("refs/containers/log-a.container", "verify-scan"),
            ("active/default/queue.wal", "wal-replay"),
            (
                "refs/containers/pointer-index-a.container",
                "pointer-index-replay",
            ),
            (
                "refs/containers/received-index-a.container",
                "received-index-replay",
            ),
            ("trust/policy-a.container", "trust-policy-replay"),
        ] {
            // Built from components, so the separators are the platform's (the message names the path as `Path::display` prints it).
            let path: std::path::PathBuf = relative.split('/').collect();
            let path = path.as_path();
            let printed = path.display().to_string();
            let refused = catch_unwind(AssertUnwindSafe(|| read_file_if_exists(mutation, path)));
            let payload = refused.expect_err(relative);
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_default();
            assert!(
                message.contains("outside every declared scope") && message.contains(&printed),
                "{relative}: {message}"
            );
            {
                let _scope = declare(scope_id);
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
