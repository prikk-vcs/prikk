//! 0.52.0 step 1 Part C (RFC 142 §4, stikk 020's own "the cost stays one replay per invocation"):
//! `show` on a 64-patch block, release build, before and after per-patch resolution. Not a
//! pass/fail assertion -- the report records the actual numbers from two separate runs of this
//! same file (one per commit) under `--nocapture`.
//!
//! `#[ignore]`d like every other heavy measurement instrument in this crate (see
//! `rfc155_archive_export_memory.rs`'s own doc comment): it needs a release build
//! (`release_binary_path()` panics under a plain `cargo test`), so the ordinary gate suite must
//! not run it. Run deliberately: `cargo test --release -p prikk --test
//! rfc142_show_block_64_patch_cost -- --ignored --nocapture`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "../support/mod.rs"]
mod support;

use std::time::Instant;

use support::*;

#[test]
#[ignore]
fn show_on_a_64_patch_block_release_build() {
    let bin = release_binary_path();
    let repo = unique_repo("rfc142-64-patch-cost");
    init(&repo);

    for k in 1..=64 {
        std::fs::write(repo.join("f.txt"), format!("line {k}\n")).unwrap();
        ok(
            &commit(&repo, "heads/main", &format!("patch {k}")),
            "commit",
        );
    }
    ok(&seal(&repo, "heads/main"), "seal");

    let block_id = tip(&repo, "heads/main");

    let mut samples = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let output = std::process::Command::new(bin)
            .current_dir(&repo)
            .args(["show", &block_id.to_string()])
            .output()
            .unwrap();
        let elapsed = start.elapsed();
        assert!(output.status.success(), "show must succeed");
        samples.push(elapsed);
    }

    eprintln!("64-patch block show, release build, 5 samples:");
    for (index, sample) in samples.iter().enumerate() {
        eprintln!("  sample {}: {:?}", index + 1, sample);
    }

    let _ = std::fs::remove_dir_all(&repo);
}

fn tip(repo: &std::path::Path, ref_name: &str) -> prikk_object::ObjectId {
    use prikk_object::{ObjectType, RefStatePayload};
    use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_state_id = RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap();
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .unwrap()
        .target_object_id
}
