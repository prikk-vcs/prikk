//! DC-78 §D4/§D6/ruling 4 — history exchange via `bundle export`/`bundle import`.
//!
//! The decisive guarantees from `implementation-handoff-v2.md` §4: import must never advance or
//! create a local ref (every `heads/*`/`tags/*` byte-identical before and after), and the bundle must
//! be a verifiable subset — the receiver gains confidence from ordinary, unmodified `verify`, not a
//! new check. This test proves both against two genuinely independent, separately-initialized
//! repositories (not one repo copied into a second directory), plus ruling 4's namespace-aware
//! presentation across `branch list`, `verify`, and `log --ref`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;

mod support;

use prikk_store::{RefStore, RepositoryLayout};

fn local_ref_pointers(repo: &Path) -> Vec<prikk_store::RefPointerSummary> {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    RefStore::new(layout).list_ref_pointers().unwrap()
}

fn bundle_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "dc78-bundle-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

#[test]
fn import_never_advances_a_local_ref_and_verify_reports_it_untrusted_until_adopted() {
    let source = support::unique_repo("bundle-source");
    support::init(&source);
    support::generation(&source, "heads/main", "a.txt", b"first\n", "first");
    support::generation(&source, "heads/main", "b.txt", b"second\n", "second");

    let bundle = bundle_path("exchange");
    let export = support::prikk(&source)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&export, "bundle export");
    let export_stdout = String::from_utf8_lossy(&export.stdout);
    assert!(
        export_stdout.contains("objects: 9"),
        "expected 2 RefStates (the full previous_ref_state_id publication chain, not only the tip) \
         + 2 Blocks + 2 Patches + 2 Blobs + the root checkpoint's snapshot Blob (each generation's CreateFile references its own Blob, \
         discovered by decoding the Patch's own operations, not just each Block's \
         snapshot_blob_ref): {export_stdout}"
    );

    let target = support::unique_repo("bundle-target");
    support::init(&target);
    let before = local_ref_pointers(&target);
    assert!(
        before.is_empty(),
        "fresh repo must start with no local refs"
    );

    let import = support::prikk(&target)
        .args(["bundle", "import", "--input", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&import, "bundle import");
    let import_stdout = String::from_utf8_lossy(&import.stdout);
    assert!(
        import_stdout.contains("received remotes/heads/main"),
        "import must report the received namespace name: {import_stdout}"
    );
    assert!(
        import_stdout.contains("new objects: 9"),
        "first import of every object must be new: {import_stdout}"
    );

    // The decisive negative control: import must not create or advance any local ref.
    let after = local_ref_pointers(&target);
    assert_eq!(
        before, after,
        "bundle import must never write to refs/by-id/, only to the received namespace"
    );

    // Before trust: verify must show the received ref and its objects, but flag them untrusted —
    // import writes objects, never trust. Do not use support::ok here; a nonzero exit is expected.
    let verify_before_trust = support::verify(&target);
    let verify_before_stdout = String::from_utf8_lossy(&verify_before_trust.stdout);
    let verify_before_stderr = String::from_utf8_lossy(&verify_before_trust.stderr);
    assert!(
        !verify_before_trust.status.success(),
        "verify must fail while the imported history's sealing key is untrusted: stdout={verify_before_stdout} stderr={verify_before_stderr}"
    );
    assert!(
        verify_before_stdout.contains("received refs: 1"),
        "verify must report the received ref: stdout={verify_before_stdout} stderr={verify_before_stderr}"
    );
    assert!(
        verify_before_stdout.contains("received-ref remotes/heads/main:"),
        "verify must name the received ref: {verify_before_stdout}"
    );
    assert!(
        !verify_before_stdout.contains("publication trust issues: 0"),
        "the imported blocks' sealing key is not yet trusted here, so publication trust issues \
         must be nonzero: {verify_before_stdout}"
    );

    // branch list must show the received ref, clearly marked, never conflated with a local branch.
    let branch_list = support::prikk(&target)
        .args(["branch", "list"])
        .output()
        .unwrap();
    support::ok(&branch_list, "branch list");
    let branch_list_stdout = String::from_utf8_lossy(&branch_list.stdout);
    assert!(
        branch_list_stdout.contains("remotes/heads/main")
            && branch_list_stdout.contains("(received)"),
        "branch list must show the received ref distinctly: {branch_list_stdout}"
    );

    // log --ref must resolve the received namespace directly, without going through RefStore.
    let log = support::prikk(&target)
        .args(["log", "--ref", "remotes/heads/main"])
        .output()
        .unwrap();
    support::ok(&log, "log --ref remotes/heads/main");
    let log_stdout = String::from_utf8_lossy(&log.stdout);
    assert!(
        log_stdout.contains("remotes/heads/main"),
        "log --ref must resolve the received ref, not report an empty/unknown ref: {log_stdout}"
    );

    // Adopt the key that sealed the source repository's history, then verify must pass cleanly and
    // attribute the imported blocks to it (DC-78 §D3, proving Stage 2 and Stage 3 compose).
    support::trust_maintainer(&target);
    let verify_after_trust = support::verify(&target);
    support::ok(&verify_after_trust, "verify after adopting the sealing key");
    let verify_after_stdout = String::from_utf8_lossy(&verify_after_trust.stdout);
    assert!(
        verify_after_stdout.contains("publication trust issues: 0"),
        "trusting the sealing key must clear every publication trust issue: {verify_after_stdout}"
    );
    assert!(
        verify_after_stdout.contains("sealed-block"),
        "verify must still attribute the imported blocks to a sealing key: {verify_after_stdout}"
    );

    // Re-importing the identical bundle must be idempotent and still not touch local refs.
    let reimport = support::prikk(&target)
        .args(["bundle", "import", "--input", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&reimport, "reimport");
    let reimport_stdout = String::from_utf8_lossy(&reimport.stdout);
    assert!(
        reimport_stdout.contains("new objects: 0"),
        "reimporting the identical bundle must write nothing new: {reimport_stdout}"
    );
    let after_reimport = local_ref_pointers(&target);
    assert_eq!(
        after, after_reimport,
        "reimport must not touch local refs either"
    );

    let _ = std::fs::remove_dir_all(source);
    let _ = std::fs::remove_dir_all(target);
    let _ = std::fs::remove_file(bundle);
}
