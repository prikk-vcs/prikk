//! RFC 136 increment 1b (handoff v1 §2): bytes per checkpoint snapshot on the RFC 139 corpus
//! profile, §10.4's first number. `#[ignore]`d, like `build_cost_curve.rs`: it builds real
//! repositories with the `prikk` binary, so it is a deliberately invoked measurement, not a
//! correctness test. Run with `cargo test -p prikk-corpus --test snapshot_bytes -- --ignored
//! --nocapture`.
//!
//! **Where the checkpoint is read.** A history of 65 blocks has its second checkpoint at the tip, and
//! one of 129 its third; `prepare_snapshot_checkout_plan` reads the tip's snapshot, so each depth is
//! its own build. The Blob's size is its stored canonical payload -- the manifest plus the Blob
//! payload's own framing.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_corpus::{Profile, execute};
use prikk_store::{
    FileObjectStore, ObjectReader, RepositoryLayout, prepare_snapshot_checkout_plan,
};

mod support;

fn self_profile() -> Profile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/profiles/prikk-self.toml"
    ))
    .expect("reading profiles/prikk-self.toml");
    toml::from_str(&text).expect("parsing profiles/prikk-self.toml")
}

#[test]
#[ignore = "expensive: builds two corpus repositories with the prikk binary"]
fn snapshot_bytes_per_checkpoint_on_the_prikk_self_profile() {
    let profile = self_profile();
    let binary = support::prikk_binary_path();
    let identity = execute::binary_identity(binary).expect("binary identity");
    eprintln!(
        "binary: {} ({}), sha256 {}",
        identity.path, identity.version_output, identity.sha256
    );
    for depth in [65_u64, 129] {
        let manifest = prikk_corpus::plan(&profile, depth).expect("plan");
        let repo_root = support::unique_dir(&format!("snapshot-bytes-{depth}"));
        execute::build(binary, &repo_root, &profile, &manifest).expect("build");
        let layout = RepositoryLayout::open(repo_root.clone()).expect("open");
        let plan = prepare_snapshot_checkout_plan(&layout, execute::REF_NAME)
            .expect("the tip is a checkpoint");
        let envelope = FileObjectStore::new(layout)
            .read_object(plan.snapshot_blob_id)
            .expect("read snapshot Blob")
            .expect("snapshot Blob present");
        let bytes = envelope.canonical_payload.len();
        #[allow(clippy::cast_precision_loss)]
        let per_entry = bytes as f64 / plan.file_count.max(1) as f64;
        eprintln!(
            "depth {depth}: {} entries, {bytes} bytes stored ({per_entry:.1} bytes/entry), \
             {} bytes of file content the manifest refers to",
            plan.file_count, plan.total_content_bytes
        );
        let _ = std::fs::remove_dir_all(repo_root);
    }
}
