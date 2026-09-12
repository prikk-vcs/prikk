//! DC-78 §D3 — `verify` must surface, per block, which MAINTAINER key sealed it. The key id already
//! lives inside the block's own signature; this is reporting that fact, not new state
//! (`rfcs/done/DC-78-HISTORY-EXCHANGE.md` §D3).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;
use std::process::Output;

mod support;

use prikk_store::{Ed25519MaintainerSigner, MaintainerSigner};

const SECOND_KEY_ID: &str = "dc78-provenance-second-maintainer";
const SECOND_SEED: [u8; 32] = [
    0x61, 0x61, 0x72, 0x72, 0x83, 0x83, 0x94, 0x94, 0xa5, 0xa5, 0xb6, 0xb6, 0xc7, 0xc7, 0xd8, 0xd8,
    0xe9, 0xe9, 0xfa, 0xfa, 0x0b, 0x0b, 0x1c, 0x1c, 0x2d, 0x2d, 0x3e, 0x3e, 0x4f, 0x4f, 0x50, 0x50,
];

fn second_public_key_hex() -> String {
    let signer = Ed25519MaintainerSigner::from_seed(SECOND_KEY_ID, &SECOND_SEED)
        .expect("fixed test seed derives a valid signer");
    support::hex(&signer.public_key_bytes())
}

fn adopt_second_key(repo: &Path) -> Output {
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

/// The precise D3 demonstration: two blocks, sealed by two different keys, must each be attributed
/// to the key that *actually* sealed them — not just "verify reports some key id somewhere," but the
/// right id against the right block. A reporting bug that always names the first adopted key
/// (regardless of who actually signed) would pass a looser assertion; this one would not.
#[test]
fn verify_attributes_each_block_to_the_maintainer_key_that_actually_sealed_it() {
    let repo = support::unique_repo("seal-provenance");
    support::init(&repo);

    std::fs::write(repo.join("a.txt"), b"first\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "first"),
        "commit first",
    );
    let first_seal = support::seal(&repo, "heads/main");
    support::ok(&first_seal, "seal first (fixed key)");
    let first_block_id = extract_block_id(&first_seal);

    support::ok(
        &adopt_second_key(&repo),
        "trust maintainer add (second key)",
    );

    std::fs::write(repo.join("b.txt"), b"second\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "second"),
        "commit second",
    );
    let second_seal = seal_with_second_key(&repo, "heads/main");
    support::ok(&second_seal, "seal second (second key)");
    let second_block_id = extract_block_id(&second_seal);

    assert_ne!(
        first_block_id, second_block_id,
        "the two seals must produce distinct blocks"
    );

    let verify_output = support::verify(&repo);
    support::ok(&verify_output, "verify");
    let stdout = String::from_utf8_lossy(&verify_output.stdout);

    assert!(
        stdout.contains(&format!(
            "sealed-block {first_block_id}: {}",
            support::MAINTAINER_KEY_ID
        )),
        "the first block must be attributed to the fixed maintainer key, not swapped: {stdout}"
    );
    assert!(
        stdout.contains(&format!("sealed-block {second_block_id}: {SECOND_KEY_ID}")),
        "the second block must be attributed to the second maintainer key, not swapped: {stdout}"
    );
    assert!(
        stdout.contains("sealed blocks: 2"),
        "expected exactly two sealed-block entries: {stdout}"
    );

    let _ = std::fs::remove_dir_all(repo);
}
