//! RFC 156 Stage 0: does a released binary still open, read and verify a repository holding a
//! **superseding record** — a second record for an existing object id whose envelope carries the stored
//! signatures plus one more valid one?
//!
//! A measurement instrument, not a correctness test: `#[ignore]`d, and it needs the released binary
//! named explicitly, because the question is about *that* binary, not a local build of its commit.
//!
//! ```text
//! PRIKK_RELEASED_BIN=/path/to/prikk-0.44.0 \
//!   cargo test -p prikk --locked --test rfc156_stage0_superseding_record_compatibility -- --ignored --nocapture
//! ```
//!
//! It prints, for both the released binary and this tree's, every command's exit code and output. The
//! state is built through the real CLI (`init`, `commit`, `seal`, `trust maintainer add`) plus one
//! test-support append that bypasses the write decision; nothing else is hand-made.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use prikk_object::ObjectId;
use prikk_store::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader,
    RepositoryLayout, append_superseding_record_for_test_support, author_signature,
    maintainer_signature,
};

const SECOND_AUTHOR_KEY_ID: &str = "stage0-second-author";
const SECOND_AUTHOR_SEED: [u8; 32] = [0x5a; 32];
const SECOND_MAINTAINER_KEY_ID: &str = "stage0-second-maintainer";
/// Sorts **before** the harness's `dc67-test-author` in canonical signature order (key id bytes first),
/// so a reader that reports "the first AUTHOR signature" reports this one, not the patch's original author.
const EARLIER_AUTHOR_KEY_ID: &str = "aaa-stage0-earlier-author";
const EARLIER_AUTHOR_SEED: [u8; 32] = [0x7c; 32];
const SECOND_MAINTAINER_SEED: [u8; 32] = [0x6b; 32];

/// A command for `binary` in `repo`, with the harness's isolated key environment and both of the
/// harness's fixed keys available — identical for the released and the current binary.
fn command(binary: &Path, repo: &Path) -> Command {
    let mut cmd = Command::new(binary);
    cmd.current_dir(repo);
    support::isolate_key_environment_for(&mut cmd, Some(repo));
    cmd.env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        );
    cmd
}

fn run(binary: &Path, repo: &Path, args: &[&str]) -> Output {
    command(binary, repo).args(args).output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn ok(output: &Output, what: &str) {
    assert!(output.status.success(), "{what}: {}", text(output));
}

/// `patch id: <hex>` from `commit`'s output.
fn patch_id_from(output: &Output) -> ObjectId {
    let line = text(output)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .map(str::trim)
        .map(str::to_owned)
        .expect("commit prints a patch id");
    line.parse().expect("a patch id")
}

/// The first `"block_id"` in `log --format json`: the tip.
fn tip_block_id(binary: &Path, repo: &Path) -> ObjectId {
    let log = text(&run(
        binary,
        repo,
        &["log", "--ref", "heads/main", "--format", "json"],
    ));
    let start = log.find("\"block_id\": \"").expect("log names a block") + "\"block_id\": \"".len();
    log[start..start + 64].parse().expect("a block id")
}

fn read_envelope(repo: &Path, id: ObjectId) -> prikk_object::ObjectEnvelope {
    let layout = RepositoryLayout::open(repo.to_path_buf()).expect("open");
    FileObjectStore::new(layout)
        .read_object(id)
        .expect("read")
        .expect("present")
}

/// Case A: a Patch whose superseding record carries two valid AUTHOR signatures, both keys recorded.
fn patch_case(current: &Path, supersede: bool) -> (PathBuf, ObjectId) {
    let repo = support::unique_repo("rfc156-stage0-patch");
    ok(&run(current, &repo, &["init"]), "init");
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("a.txt"), b"alpha\n").unwrap();
    let first = run(
        current,
        &repo,
        &["commit", "--ref", "heads/main", "-m", "a"],
    );
    ok(&first, "commit a");
    let patch_id = patch_id_from(&first);
    ok(
        &run(
            current,
            &repo,
            &["seal", "--allow-no-audit", "--ref", "heads/main"],
        ),
        "seal a",
    );

    // Record the second author's key the ordinary way: a commit of its own.
    std::fs::write(repo.join("b.txt"), b"beta\n").unwrap();
    let second_seed = support::seed_file(&support::hex(&SECOND_AUTHOR_SEED));
    ok(
        &command(current, &repo)
            .env("PRIKK_AUTHOR_KEY_ID", SECOND_AUTHOR_KEY_ID)
            .env("PRIKK_AUTHOR_SEED_FILE", &second_seed)
            .args(["commit", "--ref", "heads/main", "-m", "b"])
            .output()
            .unwrap(),
        "commit b as the second author",
    );
    ok(
        &run(
            current,
            &repo,
            &["seal", "--allow-no-audit", "--ref", "heads/main"],
        ),
        "seal b",
    );

    if !supersede {
        return (repo, patch_id);
    }
    let mut envelope = read_envelope(&repo, patch_id);
    let second = Ed25519AuthorSigner::from_seed(SECOND_AUTHOR_KEY_ID, &SECOND_AUTHOR_SEED).unwrap();
    envelope
        .add_signature(author_signature(&second, patch_id).unwrap())
        .unwrap();
    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    append_superseding_record_for_test_support(&layout, &envelope).unwrap();
    println!(
        "patch case: {} holds patch {patch_id} with {} signatures in its superseding record",
        repo.display(),
        envelope.signatures.len()
    );
    (repo, patch_id)
}

/// Case B: a Block whose superseding record carries two valid MAINTAINER signatures, both keys adopted.
/// `identical`: append the stored envelope unchanged, isolating "a second record" from "different bytes".
fn block_case(current: &Path, supersede: bool, identical: bool) -> (PathBuf, ObjectId) {
    let repo = support::unique_repo("rfc156-stage0-block");
    ok(&run(current, &repo, &["init"]), "init");
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("a.txt"), b"alpha\n").unwrap();
    ok(
        &run(
            current,
            &repo,
            &["commit", "--ref", "heads/main", "-m", "a"],
        ),
        "commit",
    );
    ok(
        &run(
            current,
            &repo,
            &["seal", "--allow-no-audit", "--ref", "heads/main"],
        ),
        "seal",
    );
    let block_id = tip_block_id(current, &repo);

    let second =
        Ed25519MaintainerSigner::from_seed(SECOND_MAINTAINER_KEY_ID, &SECOND_MAINTAINER_SEED)
            .unwrap();
    ok(
        &run(
            current,
            &repo,
            &[
                "trust",
                "maintainer",
                "add",
                "--key-id",
                SECOND_MAINTAINER_KEY_ID,
                "--public-key",
                &support::hex(&second.public_key_bytes()),
            ],
        ),
        "adopt the second maintainer",
    );

    if !supersede {
        return (repo, block_id);
    }
    let mut envelope = read_envelope(&repo, block_id);
    if identical {
        let layout = RepositoryLayout::open(repo.clone()).unwrap();
        append_superseding_record_for_test_support(&layout, &envelope).unwrap();
        println!(
            "block case (identical second record): {} holds block {block_id}",
            repo.display()
        );
        return (repo, block_id);
    }
    envelope
        .add_signature(
            maintainer_signature(&second, prikk_object::ObjectType::Block, block_id).unwrap(),
        )
        .unwrap();
    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    append_superseding_record_for_test_support(&layout, &envelope).unwrap();
    println!(
        "block case: {} holds block {block_id} with {} signatures in its superseding record",
        repo.display(),
        envelope.signatures.len()
    );
    (repo, block_id)
}

/// Case C: a RenamePath patch — the one operation 0.44.0 names an AUTHOR for (`show`'s
/// `author_key_id`) — whose superseding record adds a valid AUTHOR signature by a key that sorts
/// **first** in canonical order. Which key does `show` then name?
fn rename_case(current: &Path) -> (PathBuf, ObjectId) {
    let repo = support::unique_repo("rfc156-stage0-rename");
    ok(&run(current, &repo, &["init"]), "init");
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("a.txt"), b"alpha\n").unwrap();
    ok(
        &run(
            current,
            &repo,
            &["commit", "--ref", "heads/main", "-m", "a"],
        ),
        "commit a",
    );
    ok(
        &run(
            current,
            &repo,
            &["seal", "--allow-no-audit", "--ref", "heads/main"],
        ),
        "seal a",
    );

    // Record the earlier-sorting author's key the ordinary way: a commit of its own.
    std::fs::write(repo.join("b.txt"), b"beta\n").unwrap();
    let earlier_seed = support::seed_file(&support::hex(&EARLIER_AUTHOR_SEED));
    ok(
        &command(current, &repo)
            .env("PRIKK_AUTHOR_KEY_ID", EARLIER_AUTHOR_KEY_ID)
            .env("PRIKK_AUTHOR_SEED_FILE", &earlier_seed)
            .args(["commit", "--ref", "heads/main", "-m", "b"])
            .output()
            .unwrap(),
        "commit b as the earlier-sorting author",
    );
    ok(
        &run(
            current,
            &repo,
            &["seal", "--allow-no-audit", "--ref", "heads/main"],
        ),
        "seal b",
    );

    // The rename, authored by the harness's own author key.
    ok(&run(current, &repo, &["mv", "a.txt", "c.txt"]), "mv");
    let renamed = run(
        current,
        &repo,
        &["commit", "--ref", "heads/main", "-m", "rename"],
    );
    ok(&renamed, "commit the rename");
    let patch_id = patch_id_from(&renamed);
    ok(
        &run(
            current,
            &repo,
            &["seal", "--allow-no-audit", "--ref", "heads/main"],
        ),
        "seal the rename",
    );

    let mut envelope = read_envelope(&repo, patch_id);
    let earlier =
        Ed25519AuthorSigner::from_seed(EARLIER_AUTHOR_KEY_ID, &EARLIER_AUTHOR_SEED).unwrap();
    envelope
        .add_signature(author_signature(&earlier, patch_id).unwrap())
        .unwrap();
    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    append_superseding_record_for_test_support(&layout, &envelope).unwrap();
    println!(
        "rename case: {} holds rename patch {patch_id}; superseding record signers in canonical order: {:?}",
        repo.display(),
        envelope
            .signatures
            .iter()
            .map(|signature| signature.key_id.as_str())
            .collect::<Vec<_>>()
    );
    (repo, patch_id)
}

/// Every command the handoff names, against a copy of `source` so both binaries see the same state.
fn measure(label: &str, binary: &Path, source: &Path, id: ObjectId) {
    let repo = support::unique_repo(&format!("rfc156-stage0-{label}"));
    copy_tree(source, &repo);
    let id_text = id.to_string();
    let bundle = repo.join("stage0.bundle");
    let bundle_text = bundle.to_str().unwrap().to_string();
    let commands: Vec<Vec<&str>> = vec![
        vec!["--version"],
        vec!["verify"],
        vec!["verify", "--format", "json"],
        vec!["log", "--ref", "heads/main", "--format", "json"],
        vec!["show", &id_text, "--format", "json"],
        vec!["status"],
        vec!["doctor"],
        vec![
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            &bundle_text,
        ],
    ];
    for args in &commands {
        let output = run(binary, &repo, args);
        println!(
            "\n===== {label} :: prikk {} :: exit {:?} =====\n{}",
            args.join(" "),
            output.status.code(),
            text(&output)
        );
    }

    let receiver = support::unique_repo(&format!("rfc156-stage0-{label}-receiver"));
    let first_hex = support::maintainer_public_key_hex();
    let second_hex = support::hex(
        &Ed25519MaintainerSigner::from_seed(SECOND_MAINTAINER_KEY_ID, &SECOND_MAINTAINER_SEED)
            .unwrap()
            .public_key_bytes(),
    );
    // Both maintainer keys are adopted on the receiver, so a `verify` failure there is about the
    // imported objects, never about trust the receiver simply lacks.
    for args in [
        vec!["init"],
        vec![
            "trust",
            "maintainer",
            "add",
            "--key-id",
            support::MAINTAINER_KEY_ID,
            "--public-key",
            &first_hex,
        ],
        vec![
            "trust",
            "maintainer",
            "add",
            "--key-id",
            SECOND_MAINTAINER_KEY_ID,
            "--public-key",
            &second_hex,
        ],
        vec!["bundle", "import", "--input", &bundle_text],
        vec!["verify"],
    ] {
        let output = run(binary, &receiver, &args);
        println!(
            "\n===== {label} :: receiver :: prikk {} :: exit {:?} =====\n{}",
            args.join(" "),
            output.status.code(),
            text(&output)
        );
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
#[ignore = "RFC 156 Stage 0 measurement instrument; needs PRIKK_RELEASED_BIN"]
fn rfc156_stage0_superseding_record_compatibility() {
    let released = PathBuf::from(
        std::env::var("PRIKK_RELEASED_BIN")
            .expect("set PRIKK_RELEASED_BIN to the released prikk binary to measure"),
    );
    let current = PathBuf::from(env!("CARGO_BIN_EXE_prikk"));

    // Each case twice: with the superseding record, and a control built the same way without it, so
    // every exit code below can be attributed to the record rather than to the fixture.
    let (patch_repo, patch_id) = patch_case(&current, true);
    let (patch_control, patch_control_id) = patch_case(&current, false);
    let (block_repo, block_id) = block_case(&current, true, false);
    let (block_control, block_control_id) = block_case(&current, false, false);
    let (block_identical, block_identical_id) = block_case(&current, true, true);
    let (rename_repo, rename_id) = rename_case(&current);

    for (name, binary) in [("released", &released), ("current", &current)] {
        measure(&format!("patch-{name}"), binary, &patch_repo, patch_id);
        measure(
            &format!("patch-control-{name}"),
            binary,
            &patch_control,
            patch_control_id,
        );
        measure(&format!("block-{name}"), binary, &block_repo, block_id);
        measure(
            &format!("block-control-{name}"),
            binary,
            &block_control,
            block_control_id,
        );
        measure(
            &format!("block-identical-{name}"),
            binary,
            &block_identical,
            block_identical_id,
        );
        measure(&format!("rename-{name}"), binary, &rename_repo, rename_id);
        let prose = run(binary, &rename_repo, &["show", &rename_id.to_string()]);
        println!(
            "\n===== rename-{name} :: prikk show <id> (prose) :: exit {:?} =====\n{}",
            prose.status.code(),
            text(&prose)
        );
    }
}
