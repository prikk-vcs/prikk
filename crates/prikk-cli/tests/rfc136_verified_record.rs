//! RFC 136 increment 2b §3, on the binary: which commands write the replay-verified block record.
//! `seal` does; `branch create` and `bundle import` never do, because neither replays a root.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;

const RECORD: &str = ".prikk/cache/replay-verified-blocks.v1";

fn record(repo: &Path) -> Option<Vec<u8>> {
    std::fs::read(repo.join(RECORD)).ok()
}

#[test]
fn seal_writes_the_record_and_branch_create_and_bundle_import_do_not() {
    let source = support::unique_repo("rfc136-2b-record-source");
    support::init(&source);
    assert!(
        record(&source).is_none(),
        "fixture sanity: a new repository has no record"
    );
    support::generation(&source, "heads/main", "a.txt", b"alpha\n", "genesis");
    let after_seal = record(&source).expect("seal writes the record");
    assert!(!after_seal.is_empty());

    support::ok(
        &support::branch_create(&source, "heads/side", "heads/main"),
        "branch create",
    );
    assert_eq!(
        record(&source),
        Some(after_seal),
        "branch create leaves the record unchanged"
    );

    let bundle = source.join("main.bundle");
    support::ok(
        &support::prikk(&source)
            .args([
                "bundle",
                "export",
                "--ref",
                "heads/main",
                "--output",
                bundle.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "bundle export",
    );
    let target = support::unique_repo("rfc136-2b-record-target");
    support::init(&target);
    support::ok(
        &support::prikk(&target)
            .args(["bundle", "import", "--input", bundle.to_str().unwrap()])
            .output()
            .unwrap(),
        "bundle import",
    );
    assert!(record(&target).is_none(), "bundle import writes no record");
    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&target);
}
