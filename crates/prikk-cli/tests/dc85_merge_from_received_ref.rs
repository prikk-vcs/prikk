//! DC-85 — merging from a received ref. The decisive guarantee, per the RFC's §3A.1 mandatory
//! acceptance criterion: every Block a merge would adopt from a received ref must carry a
//! currently-trusted MAINTAINER signature, checked **before** `into_ref` advances, never deferred to
//! a later `verify` run. Local-to-local merges are safe by induction over every prior local
//! seal/merge; a received ref's blocks arrived via `import_bundle`, which performs no trust check at
//! all (DC-78 Stage 3 §4, deliberate) — so that induction does not hold, and this is the test that
//! proves the gap DC-78 Stage 3's review found is now actually closed, not merely designed around.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;
use std::process::Output;

mod support;

use prikk_store::{Ed25519MaintainerSigner, MaintainerSigner, RefStore, RepositoryLayout};

const SECOND_KEY_ID: &str = "dc85-second-maintainer";
const SECOND_SEED: [u8; 32] = [0x44_u8; 32];

fn second_public_key_hex() -> String {
    let signer = Ed25519MaintainerSigner::from_seed(SECOND_KEY_ID, &SECOND_SEED)
        .expect("fixed test seed derives a valid signer");
    support::hex(&signer.public_key_bytes())
}

fn trust_second_key(repo: &Path) -> Output {
    support::prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            SECOND_KEY_ID,
            "--public-key",
            &second_public_key_hex(),
        ])
        .output()
        .unwrap()
}

fn seal_with_second_key(repo: &Path, ref_name: &str) -> Output {
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", SECOND_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&SECOND_SEED)),
        )
        .args(["seal", "--allow-no-audit", "--ref", ref_name])
        .output()
        .unwrap()
}

fn extract_block_id(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("block id: "))
        .expect("seal output must include a block id line")
        .trim()
        .to_string()
}

fn bundle_export(repo: &Path, ref_name: &str, output_path: &Path) -> Output {
    support::prikk(repo)
        .args([
            "bundle",
            "export",
            "--ref",
            ref_name,
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

fn bundle_import(repo: &Path, input_path: &Path) -> Output {
    support::prikk(repo)
        .args(["bundle", "import", "--input", input_path.to_str().unwrap()])
        .output()
        .unwrap()
}

fn merge(
    repo: &Path,
    key_id: &str,
    seed_hex: &str,
    baseline: &str,
    into_ref: &str,
    from_ref: &str,
) -> Output {
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", key_id)
        .env("PRIKK_MAINTAINER_SEED_FILE", support::seed_file(seed_hex))
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            baseline,
            "--into",
            into_ref,
            "--from",
            from_ref,
        ])
        .output()
        .unwrap()
}

fn current_ref_state_id(repo: &Path, ref_name: &str) -> Option<prikk_object::ObjectId> {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
}

fn bundle_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "dc85-bundle-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

/// Build a genesis-shared pair: `origin` seals a genesis generation on `heads/main` with the fixed
/// test maintainer key, and `target` is a real copy of `origin`'s `.prikk` at that exact point — so
/// both repositories share the identical genesis Block id and the identical trust store, exactly the
/// way a real "clone, then diverge" starting point would. Returns `(origin, target, genesis_block_id)`.
fn shared_baseline(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let origin = support::unique_repo(&format!("dc85-{tag}-origin"));
    support::init(&origin);
    std::fs::write(origin.join("genesis.txt"), b"shared genesis content\n").unwrap();
    support::ok(
        &support::commit(&origin, "heads/main", "genesis"),
        "genesis commit",
    );
    let genesis_seal = support::seal(&origin, "heads/main");
    support::ok(&genesis_seal, "genesis seal");
    let genesis_block_id = extract_block_id(&genesis_seal);

    let target = support::unique_repo(&format!("dc85-{tag}-target"));
    std::fs::create_dir_all(target.join(".prikk")).unwrap();
    support::copy_dir_recursive(&origin.join(".prikk"), &target.join(".prikk"));

    (origin, target, genesis_block_id)
}

/// The decisive test. `origin` adopts a **second** maintainer key after the genesis copy point and
/// seals new history with it — `target` never learns about that second key. Importing that history as
/// a received ref and attempting to merge it must be refused, and refused **before** `into_ref`
/// advances, measured directly against the object store rather than trusted from the error string.
#[test]
fn merge_from_received_ref_with_untrusted_sealing_key_is_refused_and_writes_nothing() {
    let (origin, target, genesis_block_id) = shared_baseline("negative");

    // origin adopts a key target has never heard of, and seals new history with it.
    support::ok(&trust_second_key(&origin), "trust second key in origin");
    std::fs::write(origin.join("b.txt"), b"sealed by the untrusted key\n").unwrap();
    support::ok(
        &support::commit(&origin, "heads/main", "second, untrusted-to-target"),
        "commit second generation",
    );
    support::ok(
        &seal_with_second_key(&origin, "heads/main"),
        "seal second generation with the second key",
    );

    let bundle = bundle_path("negative");
    support::ok(
        &bundle_export(&origin, "heads/main", &bundle),
        "bundle export",
    );
    support::ok(&bundle_import(&target, &bundle), "bundle import");

    let before = current_ref_state_id(&target, "heads/main");
    assert!(
        before.is_some(),
        "target's heads/main must exist before the merge attempt"
    );

    let refused = merge(
        &target,
        support::MAINTAINER_KEY_ID,
        &support::hex(&support::MAINTAINER_SEED),
        &genesis_block_id,
        "heads/main",
        "remotes/heads/main",
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "merge must refuse content sealed by a key target never trusted: stdout={} stderr={stderr}",
        String::from_utf8_lossy(&refused.stdout)
    );
    assert!(
        stderr.contains("no trusted MAINTAINER signature"),
        "the refusal must name the actual reason, not a generic failure: {stderr}"
    );

    // Measured, not asserted: into_ref's RefState id is byte-identical before and after the refused
    // attempt — "it returned an error" does not by itself prove nothing was written.
    let after = current_ref_state_id(&target, "heads/main");
    assert_eq!(
        before, after,
        "a refused merge must leave into_ref completely unchanged"
    );

    let _ = std::fs::remove_dir_all(origin);
    let _ = std::fs::remove_dir_all(target);
    let _ = std::fs::remove_file(bundle);
}

/// The positive control: the same shape, but `origin`'s new history is sealed with the key `target`
/// already trusts (copied along at the genesis point) — the merge must succeed, produce a two-parent
/// `Merge` block, and the result must verify cleanly.
#[test]
fn merge_from_received_ref_with_trusted_sealing_key_succeeds() {
    let (origin, target, genesis_block_id) = shared_baseline("positive");

    std::fs::write(origin.join("b.txt"), b"sealed by the already-trusted key\n").unwrap();
    support::generation(
        &origin,
        "heads/main",
        "c.txt",
        b"more trusted content\n",
        "second",
    );

    let bundle = bundle_path("positive");
    support::ok(
        &bundle_export(&origin, "heads/main", &bundle),
        "bundle export",
    );
    support::ok(&bundle_import(&target, &bundle), "bundle import");

    let accepted = merge(
        &target,
        support::MAINTAINER_KEY_ID,
        &support::hex(&support::MAINTAINER_SEED),
        &genesis_block_id,
        "heads/main",
        "remotes/heads/main",
    );
    support::ok(
        &accepted,
        "merge from a received ref with a trusted sealing key",
    );

    let verify_output = support::verify(&target);
    support::ok(&verify_output, "verify after merging from a received ref");
    let stdout = String::from_utf8_lossy(&verify_output.stdout);
    assert!(
        stdout.contains("publication trust issues: 0"),
        "the merged-in content is sealed by a trusted key; verify must report zero trust issues: \
         {stdout}"
    );

    let _ = std::fs::remove_dir_all(origin);
    let _ = std::fs::remove_dir_all(target);
    let _ = std::fs::remove_file(bundle);
}
