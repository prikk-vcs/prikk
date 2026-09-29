//! RFC 163 §2, §6: **a write never buries a crash state (I5).** At each of scope B's five files
//! (the pointer index, trust keys, trust policy, author keys, the received index), a torn tail is a
//! crash state `verify` accepts (RFC 162 rule 3). Before this round, the *next ordinary write* -- not
//! a repair -- appended blind and turned that accepted crash state into permanent damage (letter 015,
//! N1/N2). Now: the writer confirms under its lock that the file ends at its last sound record and
//! refuses before writing anything, naming the way out.
//!
//! Each file below gets its own setup (the "ordinary write" that triggers an append differs too much
//! per file to share one generic driver the way RFC 162's four-file matrix could -- `seal`/`branch
//! create`/`tag create` for the pointer index, `trust maintainer add` for the two trust containers, a
//! commit by a new author for author keys, `bundle import` for the received index), but every row
//! checks the same three things: the write refuses and changes nothing; the way out (a repair verb for
//! the pointer index, a manual truncate for the other four, per RFC 163 §4) reaches a repository where
//! the same write succeeds; and `verify` and a further `commit` both then succeed.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

/// magic(8) version(2) body_len(8) checksum(32) -- every one of scope B's five files shares this
/// header shape (confirmed against each module's own `*_HEADER_LEN` constant, not assumed).
const HEADER_LEN: usize = 8 + 2 + 8 + 32;

fn run(repo: &Path, args: &[&str]) -> (Option<i32>, String, Output) {
    let output = support::prikk(repo).args(args).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.code(), text, output)
}

fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("target file exists")
}

/// A torn prefix: a copy of this format's own magic+version, cut short before a full header lands --
/// appended after whatever sound content is already there, which stays intact.
fn append_torn_prefix(path: &Path) {
    let mut bytes = read_bytes(path);
    let prefix_len = (HEADER_LEN - 5).min(bytes.len());
    let prefix = bytes[..prefix_len].to_vec();
    bytes.extend(prefix);
    std::fs::write(path, &bytes).unwrap();
}

/// 100 zero bytes -- fatal before RFC 162 rule 3, a repairable/truncatable tail after it (it parses as
/// neither a sound frame nor a structurally-short one, so nothing sound follows and it is tail).
fn append_zeros_100(path: &Path) {
    let mut bytes = read_bytes(path);
    bytes.extend(vec![0_u8; 100]);
    std::fs::write(path, &bytes).unwrap();
}

/// 30 zero bytes -- fewer than one header (50 bytes), so every one of scope B's five files classifies
/// this the same way: too few bytes remain for even a header, a genuine `TrailingPartial`. Used for
/// the four files that still use the pre-0.48.0 shape rule (RFC 162 rule 3 was not extended to them
/// this round, N2) instead of 100 zero bytes: at 100 bytes (a full header's worth), those four files'
/// own `Invalid`-frame handling marks it `Failed` (interior damage) regardless of what follows --
/// unlike the pointer index, which RFC 162 rule 3 already covers -- so it is refused already, by the
/// pre-existing `has_item_failure` check, not by this round's new tail guard. 30 zero bytes isolates
/// what this round actually added: a genuine short tail, invisible to `has_item_failure`, that only
/// the new guard catches.
fn append_zeros_30(path: &Path) {
    let mut bytes = read_bytes(path);
    bytes.extend(vec![0_u8; 30]);
    std::fs::write(path, &bytes).unwrap();
}

/// A named fault: a label, and the function that applies it to a target file's path.
type FaultFn = fn(&Path);

/// Faults for the pointer index, which RFC 162 rule 3 already covers: both are genuinely repairable
/// tails there, whatever their shape.
const POINTER_INDEX_FAULTS: [(&str, FaultFn); 2] = [
    ("torn prefix", append_torn_prefix as FaultFn),
    ("100 zero bytes", append_zeros_100 as FaultFn),
];

/// Faults for the four files still on the pre-0.48.0 shape rule (trust keys, trust policy, author
/// keys, the received index) -- both genuinely short of one header, so both are `TrailingPartial`
/// there (see [`append_zeros_30`]'s own doc for why 100 zero bytes would test something else).
const SHAPE_RULE_FAULTS: [(&str, FaultFn); 2] = [
    ("torn prefix", append_torn_prefix as FaultFn),
    ("30 zero bytes", append_zeros_30 as FaultFn),
];

// ---------------------------------------------------------------------------------------------
// 1. The pointer index -- seal, branch create, tag create.
// ---------------------------------------------------------------------------------------------

fn pointer_index_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/refs/containers/pointer-index-a.container")
}

/// One commit (which publishes, so the pointer index already holds one entry) and a seal, so `branch
/// create --from heads/main` and `tag create --target heads/main` both have a real target too.
fn pointer_index_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    repo
}

/// The three publications N1's own table names, each exercised against the same corrupted pointer
/// index -- delegating to `support`'s own helpers, which each trust the maintainer first (a write to
/// the unrelated trust-key/trust-policy containers, unaffected by the pointer index's own corruption)
/// and carry the maintainer signing environment. `label` is folded into the branch/tag name so three
/// attempts against one repository do not collide.
fn pointer_index_write(repo: &Path, which: &str, label: &str) -> (Option<i32>, String) {
    fn text_of(output: &std::process::Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }
    let output = match which {
        "seal" => {
            std::fs::write(repo.join(format!("{label}.txt")), "more\n".repeat(5)).unwrap();
            support::ok(
                &support::commit(repo, "heads/main", label),
                "queue a commit before sealing",
            );
            support::seal(repo, "heads/main")
        }
        "branch create" => support::branch_create(repo, &format!("heads/{label}"), "heads/main"),
        "tag create" => support::tag_create(repo, &format!("tags/{label}"), "heads/main"),
        _ => unreachable!(),
    };
    (output.status.code(), text_of(&output))
}

#[test]
fn pointer_index_a_write_refuses_on_an_unclean_tail_then_the_repair_lets_it_through() {
    let mut failures = Vec::new();
    for (fault_name, fault) in POINTER_INDEX_FAULTS {
        for which in ["seal", "branch create", "tag create"] {
            let repo = pointer_index_repository(&format!(
                "rfc163-pointer-{}-{}",
                fault_name.replace(' ', "-"),
                which.replace(' ', "-")
            ));
            let path = pointer_index_path(&repo);
            let before = read_bytes(&path);
            fault(&path);
            let corrupted = read_bytes(&path);
            let label = format!("pointer index / {fault_name} / {which}");

            // `seal` needs the maintainer trusted first -- and trusting the maintainer publishes to
            // the pointer index too (a distinct ref name key: `refs/maintainers/...` is NOT what this
            // repository uses -- trust is its own container, not a ref publication), so it must not
            // itself be blocked by the corrupted pointer index. Confirmed: `trust_maintainer` writes
            // to the trust-key/trust-policy containers only, never the ref pointer index.
            support::trust_maintainer(&repo);

            let (code, text) = pointer_index_write(&repo, which, "attempt");
            if code.is_some_and(|code| code == 0) {
                failures.push(format!(
                    "{label}: the write must refuse on the unclean tail, but exited 0\n{text}"
                ));
            }
            assert!(
                text.contains("incomplete tail") || text.contains("pointer index"),
                "{label}: the refusal must name the pointer index and its tail\n{text}"
            );
            let after_refusal = read_bytes(&path);
            if after_refusal != corrupted {
                failures.push(format!(
                    "{label}: the refused write changed the pointer index file"
                ));
            }

            // The way out: `doctor --repair-pointer-index-tail`.
            let (repair_code, repair_text, _) =
                run(&repo, &["doctor", "--repair-pointer-index-tail"]);
            if repair_code != Some(0) {
                failures.push(format!("{label}: the repair itself\n{repair_text}"));
                let _ = std::fs::remove_dir_all(repo);
                continue;
            }

            let (retry_code, retry_text) = pointer_index_write(&repo, which, "retry");
            if retry_code != Some(0) {
                failures.push(format!(
                    "{label}: I3, the same write after the repair must succeed\n{retry_text}"
                ));
            }

            let (verify_code, verify_text, _) = run(&repo, &["verify"]);
            if verify_code != Some(0) {
                failures.push(format!(
                    "{label}: verify after the repair and retry\n{verify_text}"
                ));
            }

            std::fs::write(repo.join("after.txt"), b"after\n").unwrap();
            let commit_output = support::commit(&repo, "heads/main", "after the repair");
            if !commit_output.status.success() {
                failures.push(format!(
                    "{label}: a commit after the repair and retry: {}{}",
                    String::from_utf8_lossy(&commit_output.stdout),
                    String::from_utf8_lossy(&commit_output.stderr)
                ));
            }

            let _ = std::fs::remove_dir_all(&repo);
            let _ = before; // kept for readability of intent; the byte-identity check above is what matters
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

// ---------------------------------------------------------------------------------------------
// 2. Trust keys and trust policy -- `trust maintainer add` with a second, distinct key.
// ---------------------------------------------------------------------------------------------

const SECOND_MAINTAINER_KEY_ID: &str = "rfc163-second-maintainer";
const SECOND_MAINTAINER_SEED: [u8; 32] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
];

fn second_maintainer_public_key_hex() -> String {
    use prikk_store::MaintainerSigner;
    let signer = prikk_store::Ed25519MaintainerSigner::from_seed(
        SECOND_MAINTAINER_KEY_ID,
        &SECOND_MAINTAINER_SEED,
    )
    .expect("fixed second maintainer seed derives a valid signer");
    support::hex(&signer.public_key_bytes())
}

fn trust_key_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/keys.container")
}

fn trust_policy_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/policy-a.container")
}

fn add_second_maintainer(repo: &Path) -> (Option<i32>, String) {
    let (code, text, _) = run(
        repo,
        &[
            "trust",
            "maintainer",
            "add",
            "--key-id",
            SECOND_MAINTAINER_KEY_ID,
            "--public-key",
            &second_maintainer_public_key_hex(),
        ],
    );
    (code, text)
}

/// `trust maintainer add` for the *first* key -- what every fixture starts from, so the trust-key and
/// trust-policy containers are non-empty (one entry each) before the fault is applied.
fn trust_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    repo
}

fn trust_container_case(target_name: &str, path_of: fn(&Path) -> PathBuf) {
    let mut failures = Vec::new();
    for (fault_name, fault) in SHAPE_RULE_FAULTS {
        let repo = trust_repository(&format!(
            "rfc163-{}-{}",
            target_name.replace(' ', "-"),
            fault_name.replace(' ', "-")
        ));
        let path = path_of(&repo);
        let original_len = read_bytes(&path).len();
        fault(&path);
        let corrupted = read_bytes(&path);
        let label = format!("{target_name} / {fault_name}");

        let (code, text) = add_second_maintainer(&repo);
        if code.is_some_and(|code| code == 0) {
            failures.push(format!(
                "{label}: adding the second maintainer must refuse on the unclean tail, but exited 0\n{text}"
            ));
        }
        assert!(
            text.contains("incomplete tail"),
            "{label}: the refusal must name the unclean tail\n{text}"
        );
        if read_bytes(&path) != corrupted {
            failures.push(format!("{label}: the refused write changed the file"));
        }

        // The way out (RFC 163 §4): no repair verb in 0.48.0 for these four files -- back the file
        // up, truncate to the offset the refusal named (here: the length before the fault, which is
        // the same value by construction), then retry.
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(original_len as u64).unwrap();
        drop(file);

        let (retry_code, retry_text) = add_second_maintainer(&repo);
        if retry_code != Some(0) {
            failures.push(format!(
                "{label}: I3, the same write after the manual truncate must succeed\n{retry_text}"
            ));
        }

        let (verify_code, verify_text, _) = run(&repo, &["verify"]);
        if verify_code != Some(0) {
            failures.push(format!(
                "{label}: verify after the truncate and retry\n{verify_text}"
            ));
        }

        let _ = std::fs::remove_dir_all(&repo);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

#[test]
fn trust_keys_a_write_refuses_on_an_unclean_tail_then_a_manual_truncate_lets_it_through() {
    trust_container_case("trust keys", trust_key_path);
}

#[test]
fn trust_policy_a_write_refuses_on_an_unclean_tail_then_a_manual_truncate_lets_it_through() {
    trust_container_case("trust policy", trust_policy_path);
}

// ---------------------------------------------------------------------------------------------
// 3. Author keys -- a commit by a new author.
// ---------------------------------------------------------------------------------------------

const SECOND_AUTHOR_KEY_ID: &str = "rfc163-second-author";
const SECOND_AUTHOR_SEED_HEX: &str =
    "d279cf77ef03bdacc61cae6e365bd11fcfaed31b9226ddb9e67ab1e95450ea60";

fn author_key_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/author-keys.container")
}

fn commit_by_second_author(repo: &Path, message: &str) -> (Option<i32>, String) {
    let output = support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", SECOND_AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(SECOND_AUTHOR_SEED_HEX),
        )
        .args(["commit", "--ref", "heads/main", "-m", message])
        .output()
        .unwrap();
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// One commit by the fixed (first) author, so the author-key container is non-empty before the fault.
fn author_key_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    repo
}

#[test]
fn author_keys_a_write_refuses_on_an_unclean_tail_then_a_manual_truncate_lets_it_through() {
    let mut failures = Vec::new();
    for (fault_name, fault) in SHAPE_RULE_FAULTS {
        let repo = author_key_repository(&format!(
            "rfc163-author-keys-{}",
            fault_name.replace(' ', "-")
        ));
        let path = author_key_path(&repo);
        let original_len = read_bytes(&path).len();
        fault(&path);
        let corrupted = read_bytes(&path);
        let label = format!("author keys / {fault_name}");

        std::fs::write(repo.join("by-second-author.txt"), "second\n".repeat(5)).unwrap();
        let (code, text) = commit_by_second_author(&repo, "by the second author");
        if code.is_some_and(|code| code == 0) {
            failures.push(format!(
                "{label}: a commit by a new author must refuse on the unclean tail, but exited 0\n{text}"
            ));
        }
        assert!(
            text.contains("incomplete tail"),
            "{label}: the refusal must name the unclean tail\n{text}"
        );
        if read_bytes(&path) != corrupted {
            failures.push(format!("{label}: the refused write changed the file"));
        }

        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(original_len as u64).unwrap();
        drop(file);

        let (retry_code, retry_text) =
            commit_by_second_author(&repo, "by the second author, retry");
        if retry_code != Some(0) {
            failures.push(format!(
                "{label}: I3, the same commit after the manual truncate must succeed\n{retry_text}"
            ));
        }

        let (verify_code, verify_text, _) = run(&repo, &["verify"]);
        if verify_code != Some(0) {
            failures.push(format!(
                "{label}: verify after the truncate and retry\n{verify_text}"
            ));
        }

        let _ = std::fs::remove_dir_all(&repo);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

// ---------------------------------------------------------------------------------------------
// 4. The received index -- `bundle import`.
// ---------------------------------------------------------------------------------------------

fn received_index_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/refs/containers/received-index-a.container")
}

/// A sender repository with one sealed commit, and its bundle already exported to `<receiver>/../sender.bundle`.
fn export_a_bundle(tag: &str) -> PathBuf {
    let sender = support::unique_repo(&format!("{tag}-sender"));
    support::init(&sender);
    std::fs::write(sender.join("s.txt"), "s\n".repeat(20)).unwrap();
    support::ok(
        &support::commit(&sender, "heads/main", "sender commit"),
        "commit",
    );
    support::ok(&support::seal(&sender, "heads/main"), "seal");
    let bundle = sender.join("export.bundle");
    support::ok(
        &support::prikk(&sender)
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
    bundle
}

/// The receiver, with the bundle already imported once (so the received index already holds one
/// entry before the fault).
fn received_index_repository(tag: &str) -> (PathBuf, PathBuf) {
    let bundle = export_a_bundle(tag);
    let receiver = support::unique_repo(&format!("{tag}-receiver"));
    support::init(&receiver);
    // The sender sealed under the fixed `support::MAINTAINER_KEY_ID`; the receiver must trust the
    // same key for `verify` to accept the received history's publication trust -- unrelated to the
    // received index itself, but needed for the later `verify exits 0` check to mean anything.
    support::trust_maintainer(&receiver);
    support::ok(
        &support::prikk(&receiver)
            .args(["bundle", "import", "--input", bundle.to_str().unwrap()])
            .output()
            .unwrap(),
        "first bundle import",
    );
    (receiver, bundle)
}

#[test]
fn received_index_a_write_refuses_on_an_unclean_tail_then_a_manual_truncate_lets_it_through() {
    let mut failures = Vec::new();
    for (fault_name, fault) in SHAPE_RULE_FAULTS {
        let (repo, bundle) = received_index_repository(&format!(
            "rfc163-received-index-{}",
            fault_name.replace(' ', "-")
        ));
        let path = received_index_path(&repo);
        let original_len = read_bytes(&path).len();
        fault(&path);
        let corrupted = read_bytes(&path);
        let label = format!("received index / {fault_name}");

        // Re-importing the same bundle is still an append -- `write_received_pointer` never checks
        // for an existing entry first (last-entry-wins, no CAS), so this is a second, ordinary write.
        let (code, text, _) = run(
            &repo,
            &["bundle", "import", "--input", bundle.to_str().unwrap()],
        );
        if code.is_some_and(|code| code == 0) {
            failures.push(format!(
                "{label}: re-importing must refuse on the unclean tail, but exited 0\n{text}"
            ));
        }
        assert!(
            text.contains("incomplete tail"),
            "{label}: the refusal must name the unclean tail\n{text}"
        );
        if read_bytes(&path) != corrupted {
            failures.push(format!("{label}: the refused write changed the file"));
        }

        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(original_len as u64).unwrap();
        drop(file);

        let (retry_code, retry_text, _) = run(
            &repo,
            &["bundle", "import", "--input", bundle.to_str().unwrap()],
        );
        if retry_code != Some(0) {
            failures.push(format!(
                "{label}: I3, the same import after the manual truncate must succeed\n{retry_text}"
            ));
        }

        let (verify_code, verify_text, _) = run(&repo, &["verify"]);
        if verify_code != Some(0) {
            failures.push(format!(
                "{label}: verify after the truncate and retry\n{verify_text}"
            ));
        }

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(bundle.parent().unwrap());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}
