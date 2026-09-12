//! Shared helpers for `merge_evidence` integration tests.

#[path = "../support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, ChangePerm, CreateFile,
    MerkleRoot, NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind,
    PatchPayload, PatchPurpose, Signature, SignatureAlgorithm, SignerRole,
};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectWriter, RepositoryLayout,
};

pub(crate) type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

const MODE_REGULAR: u32 = 0o100_644;
const MODE_EXECUTABLE: u32 = 0o100_755;

pub(crate) fn prikk(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prikk"));
    cmd.current_dir(repo);
    cmd
}

pub(crate) fn ok(output: &Output, what: &str) -> TestResult {
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
    .into())
}

pub(crate) fn fail(output: &Output, what: &str) -> TestResult {
    if !output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{what} unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
    .into())
}

pub(crate) fn unique_repo(tag: &str) -> TestResult<PathBuf> {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "prikk-cli-merge-evidence-{tag}-{}",
        support::unique_suffix()
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn maintainer_seed() -> &'static str {
    "111122223333444455556666777788889999aaaabbbbccccddddeeeeffff0000"
}

fn maintainer_signer() -> TestResult<Ed25519MaintainerSigner> {
    Ok(Ed25519MaintainerSigner::from_seed(
        "merge-evidence-maintainer",
        &[
            0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0x55, 0x55, 0x66, 0x66, 0x77, 0x77,
            0x88, 0x88, 0x99, 0x99, 0xaa, 0xaa, 0xbb, 0xbb, 0xcc, 0xcc, 0xdd, 0xdd, 0xee, 0xee,
            0xff, 0xff, 0x00, 0x00,
        ],
    )?)
}

fn public_key_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn add_trusted_maintainer(repo: &Path) -> TestResult {
    let signer = maintainer_signer()?;
    let out = prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "merge-evidence-maintainer",
            "--public-key",
            &public_key_hex(&signer.public_key_bytes()),
        ])
        .output()?;
    ok(&out, "trust maintainer add")
}

pub(crate) fn commit_worktree(repo: &Path, message: &str) -> TestResult {
    let out = prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", "merge-evidence-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "-m", message])
        .output()?;
    ok(&out, "commit")
}

pub(crate) fn seal_current(repo: &Path) -> TestResult<String> {
    let out = prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "merge-evidence-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit"])
        .output()?;
    ok(&out, "seal")?;
    seal_block_id(&String::from_utf8_lossy(&out.stdout))
}

pub(crate) fn init_with_sealed_genesis(repo: &Path) -> TestResult<String> {
    let out = prikk(repo).arg("init").output()?;
    ok(&out, "init")?;
    std::fs::write(repo.join("readme.txt"), b"hello prikk\n")?;
    commit_worktree(repo, "genesis")?;
    add_trusted_maintainer(repo)?;
    seal_current(repo)
}

fn author_signature() -> Signature {
    Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: "merge-evidence-author".to_string(),
        signature_bytes: vec![1; 64],
        created_at: 7,
        signer_role: SignerRole::Author,
    }
}

fn maintainer_signature() -> Signature {
    Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: "merge-evidence-maintainer".to_string(),
        signature_bytes: vec![5; 64],
        created_at: 8,
        signer_role: SignerRole::Maintainer,
    }
}

fn write_blob(layout: &RepositoryLayout, content: &[u8]) -> TestResult<ObjectId> {
    let mut store = FileObjectStore::new(layout.clone());
    let blob = BlobPayload::new(BlobKind::Text, content.to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    Ok(store.write_object(&envelope)?)
}

fn write_patch(layout: &RepositoryLayout, operations: Vec<Operation>) -> TestResult<ObjectId> {
    let mut store = FileObjectStore::new(layout.clone());
    let patch = PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch.to_canonical_bytes()?);
    envelope.add_signature(author_signature())?;
    Ok(store.write_object(&envelope)?)
}

fn write_block(
    layout: &RepositoryLayout,
    kind: BlockKind,
    parents: Vec<ObjectId>,
    patches: Vec<ObjectId>,
) -> TestResult<ObjectId> {
    let mut store = FileObjectStore::new(layout.clone());
    let block = BlockPayload {
        parent_block_ids: parents,
        kind,
        patch_ids: patches,
        state_merkle_root: MerkleRoot([0_u8; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, block.to_canonical_bytes()?);
    envelope.add_signature(maintainer_signature())?;
    Ok(store.write_object(&envelope)?)
}

pub(crate) fn write_conflict_fixture(repo: &Path) -> TestResult<(String, String, String)> {
    let layout = RepositoryLayout::init(repo.to_path_buf())?;
    let node_id = NodeId::from_bytes([0x44; 32]);
    let blob_id = write_blob(&layout, b"conflict baseline\n")?;
    let baseline_patch = write_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "conflict.txt".to_string(),
                node_id,
                blob_id,
                mode: MODE_REGULAR,
            }),
        }],
    )?;
    let baseline = write_block(&layout, BlockKind::Root, Vec::new(), vec![baseline_patch])?;
    let left_patch = write_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ChangePerm(ChangePerm {
                node_id,
                old_mode: MODE_REGULAR,
                new_mode: MODE_EXECUTABLE,
            }),
        }],
    )?;
    let right_patch = write_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ChangePerm(ChangePerm {
                node_id,
                old_mode: MODE_REGULAR,
                // RFC 125 §2: only two file modes are canonical, so this can no longer target a
                // third, distinct value the way it once did (`MODE_PRIVATE`, 0o100_600, retired).
                // The conflict here comes from two `ChangePerm`s touching the same node
                // concurrently at all (`classify_same_node`'s `_ => UnknownRelation` fallthrough,
                // `patch_algebra/classify.rs`), not from the two sides disagreeing on the target
                // value, so targeting the same canonical mode as `left` still conflicts.
                new_mode: MODE_EXECUTABLE,
            }),
        }],
    )?;
    let left = write_block(&layout, BlockKind::Normal, vec![baseline], vec![left_patch])?;
    let right = write_block(
        &layout,
        BlockKind::Normal,
        vec![baseline],
        vec![right_patch],
    )?;
    Ok((baseline.to_string(), left.to_string(), right.to_string()))
}

fn seal_block_id(stdout: &str) -> TestResult<String> {
    let block_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("block id: "))
        .ok_or_else(|| io::Error::other("seal output did not include block id"))?;
    Ok(block_id.to_string())
}

pub(crate) fn snapshot_files(root: &Path) -> TestResult<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    collect_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_files(root: &Path, current: &Path, files: &mut BTreeMap<String, Vec<u8>>) -> TestResult {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|err| io::Error::other(err.to_string()))?
                .to_string_lossy()
                .to_string();
            files.insert(relative, std::fs::read(path)?);
        }
    }
    Ok(())
}
