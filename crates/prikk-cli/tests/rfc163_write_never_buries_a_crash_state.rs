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

/// 4,096 zero bytes -- a full header's worth and then some, so every framed reader's `Invalid`-frame
/// handling marks it damage (external review 016, N9's own second fault size).
fn append_zeros_4096(path: &Path) {
    let mut bytes = read_bytes(path);
    bytes.extend(vec![0_u8; 4096]);
    std::fs::write(path, &bytes).unwrap();
}

/// 100 bytes of a fixed (not random-per-run) byte pattern that is not all zero -- N9's third fault
/// size, chosen fixed rather than from a real RNG so a failing run reproduces exactly.
fn append_random_100(path: &Path) {
    let mut bytes = read_bytes(path);
    bytes.extend((0..100_u32).map(|index| (index.wrapping_mul(2654435761) >> 24) as u8));
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

/// Addendum 1 item 2: re-adding the repository's own, **already-adopted** maintainer key (the fixed
/// key `support::trust_maintainer` already adopted) appends nothing to either trust container -- an
/// idempotent no-op -- so it must succeed despite a torn tail on either, and must not touch either
/// file.
#[test]
fn trust_maintainer_add_of_the_already_adopted_key_is_unaffected_by_a_tail_on_either_container() {
    let mut failures = Vec::new();
    for (target_name, path_of) in [
        ("trust keys", trust_key_path as fn(&Path) -> PathBuf),
        ("trust policy", trust_policy_path as fn(&Path) -> PathBuf),
    ] {
        for (fault_name, fault) in SHAPE_RULE_FAULTS {
            let repo = trust_repository(&format!(
                "rfc163-{}-no-append-{}",
                target_name.replace(' ', "-"),
                fault_name.replace(' ', "-")
            ));
            let path = path_of(&repo);
            fault(&path);
            let corrupted = read_bytes(&path);
            let label = format!("{target_name}, already-adopted key / {fault_name}");

            let (code, text, _) = run(
                &repo,
                &[
                    "trust",
                    "maintainer",
                    "add",
                    "--key-id",
                    support::MAINTAINER_KEY_ID,
                    "--public-key",
                    &support::maintainer_public_key_hex(),
                ],
            );
            if code != Some(0) {
                failures.push(format!(
                    "{label}: re-adding the already-adopted key must succeed despite the tail\n{text}"
                ));
            }
            if read_bytes(&path) != corrupted {
                failures.push(format!(
                    "{label}: the file must be untouched -- this call appends to neither container"
                ));
            }

            let _ = std::fs::remove_dir_all(&repo);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

/// RFC 163 §2 Addendum 2 (review `release-0.48.0-candidate-3-review-v1` §2.2): a torn tail on the
/// trust **policy** container alone -- the trust-key container stays clean -- must refuse `trust
/// maintainer add` of a genuinely new key **before either container is appended to**, not append the
/// key to `trust/keys.container` and then refuse over the policy. Whole-repository byte identity
/// (`support::store_bytes`), not just the policy file's own bytes, is the point: the old, half-applied
/// order left the trust-key file changed while its own refusal said nothing was written.
#[test]
fn trust_maintainer_add_refuses_before_either_container_when_only_the_policy_tail_is_torn() {
    let mut failures = Vec::new();
    for (fault_name, fault) in SHAPE_RULE_FAULTS {
        let repo = trust_repository(&format!(
            "rfc163-trust-policy-only-torn-{}",
            fault_name.replace(' ', "-")
        ));
        // The trust-key container is deliberately left clean: this test isolates the policy-only tail.
        let policy_path = trust_policy_path(&repo);
        let original_len = read_bytes(&policy_path).len();
        fault(&policy_path);
        let label = format!("trust policy only / {fault_name}");

        let before = support::store_bytes(&repo);
        let (code, text) = add_second_maintainer(&repo);
        if code.is_some_and(|code| code == 0) {
            failures.push(format!(
                "{label}: adding a new key must refuse on the policy's unclean tail, but exited 0\n{text}"
            ));
        }
        assert!(
            text.contains("incomplete tail"),
            "{label}: the refusal must name the unclean tail\n{text}"
        );
        let after = support::store_bytes(&repo);
        if before != after {
            let mut changed: Vec<String> = before
                .keys()
                .chain(after.keys())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .filter(|path| before.get(*path) != after.get(*path))
                .map(|path| path.display().to_string())
                .collect();
            changed.sort();
            changed.dedup();
            failures.push(format!(
                "{label}: every file under .prikk/ must be byte-identical after the refusal; changed: {changed:?}"
            ));
        }

        // The way out: truncate the policy container back to its pre-fault length.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&policy_path)
            .unwrap();
        file.set_len(original_len as u64).unwrap();
        drop(file);

        let (retry_code, retry_text) = add_second_maintainer(&repo);
        if retry_code != Some(0) {
            failures.push(format!(
                "{label}: the same add after the manual truncate must succeed\n{retry_text}"
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

/// Addendum 1 item 2: a commit by the repository's own, **already-recorded** author appends nothing to
/// the author-key container, so it must succeed despite a torn tail there -- and must not touch the
/// file. Uses `support::commit` (the fixed, first author, already recorded by
/// `author_key_repository`'s own setup commit).
#[test]
fn author_keys_a_commit_by_the_already_recorded_author_is_unaffected_by_a_tail() {
    let mut failures = Vec::new();
    for (fault_name, fault) in SHAPE_RULE_FAULTS {
        let repo = author_key_repository(&format!(
            "rfc163-author-keys-no-append-{}",
            fault_name.replace(' ', "-")
        ));
        let path = author_key_path(&repo);
        fault(&path);
        let corrupted = read_bytes(&path);
        let label = format!("author keys, already-recorded author / {fault_name}");

        std::fs::write(repo.join("by-first-author.txt"), "first\n".repeat(5)).unwrap();
        let commit = support::commit(&repo, "heads/main", "by the already-recorded author");
        if !commit.status.success() {
            failures.push(format!(
                "{label}: a commit by the already-recorded author must succeed despite the tail\n{}{}",
                String::from_utf8_lossy(&commit.stdout),
                String::from_utf8_lossy(&commit.stderr)
            ));
        }
        if read_bytes(&path) != corrupted {
            failures.push(format!(
                "{label}: the author-key container must be untouched -- this commit appends nothing to it"
            ));
        }

        let _ = std::fs::remove_dir_all(&repo);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
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
    export_a_bundle_named(tag, "s.txt", "export.bundle")
}

/// Like [`export_a_bundle`], with the committed file and the bundle's own file name given explicitly --
/// so two calls with two different `file_name`s produce two bundles carrying genuinely different
/// objects (a second import of the second bundle would write *new* objects, unlike a second import of
/// the same bundle, which writes none).
fn export_a_bundle_named(tag: &str, file_name: &str, bundle_name: &str) -> PathBuf {
    let sender = support::unique_repo(&format!("{tag}-sender-{file_name}"));
    support::init(&sender);
    std::fs::write(sender.join(file_name), format!("{file_name}\n").repeat(20)).unwrap();
    support::ok(
        &support::commit(
            &sender,
            "heads/main",
            &format!("sender commit: {file_name}"),
        ),
        "commit",
    );
    support::ok(&support::seal(&sender, "heads/main"), "seal");
    let bundle = sender.join(bundle_name);
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

/// Addendum 1 item 1 (blocks): **a refused `bundle import` writes nothing at all**, not only leaving
/// the received index untouched -- 0.44.0's own guarantee (GHSA-px5q-233r-6hq5) for every other
/// refusal, which the received-index guard's first shape violated by running inside
/// `append_received_index_entry`, after the objects and author keys were already durable. The second
/// bundle carries genuinely new objects (a different sender, a different committed file), so a check
/// that fired too late would show up as new files under `.prikk/containers/`, not only a changed
/// received-index file.
#[test]
fn received_index_a_refused_import_writes_nothing_at_all() {
    let mut failures = Vec::new();
    for (fault_name, fault) in SHAPE_RULE_FAULTS {
        let tag = format!(
            "rfc163-received-index-nothing-written-{}",
            fault_name.replace(' ', "-")
        );
        let (repo, _first_bundle) = received_index_repository(&tag);
        let second_bundle = export_a_bundle_named(&tag, "second-sender-file.txt", "second.bundle");

        let path = received_index_path(&repo);
        fault(&path);
        let label = format!("received index, whole-repo / {fault_name}");

        let before = support::store_bytes(&repo);
        let (code, text, _) = run(
            &repo,
            &[
                "bundle",
                "import",
                "--input",
                second_bundle.to_str().unwrap(),
            ],
        );
        if code.is_some_and(|code| code == 0) {
            failures.push(format!(
                "{label}: the import must refuse on the unclean tail, but exited 0\n{text}"
            ));
        }
        let after = support::store_bytes(&repo);
        if before != after {
            let mut changed: Vec<String> = before
                .keys()
                .chain(after.keys())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .filter(|path| before.get(*path) != after.get(*path))
                .map(|path| path.display().to_string())
                .collect();
            changed.sort();
            changed.dedup();
            failures.push(format!(
                "{label}: every file under .prikk/ must be byte-identical after a refused import; changed: {changed:?}"
            ));
        }

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(second_bundle.parent().unwrap());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
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

// ---------------------------------------------------------------------------------------------
// 5. External review 016, N9 -- the received index refuses on a damaged entry, not only on a
//    torn tail. Before this fix, `scan_received_index_tail` resynced silently past an `Invalid`
//    frame the way a reader would, so `bundle import` appended behind already-disclosed damage
//    and buried it for good -- the one guarded writer where `verify`'s own "has a damaged entry"
//    refusal was not matched by a write-side refusal at all.
// ---------------------------------------------------------------------------------------------

const RECEIVED_INDEX_DAMAGE_FAULTS: [(&str, FaultFn); 3] = [
    ("100 zero bytes", append_zeros_100 as FaultFn),
    ("4,096 zero bytes", append_zeros_4096 as FaultFn),
    ("100 random bytes", append_random_100 as FaultFn),
];

#[test]
fn received_index_refuses_on_a_damaged_entry_before_the_first_write() {
    let mut failures = Vec::new();
    for (fault_name, fault) in RECEIVED_INDEX_DAMAGE_FAULTS {
        let (repo, bundle) = received_index_repository(&format!(
            "rfc163-received-index-damage-{}",
            fault_name.replace(' ', "-").replace(',', "")
        ));
        let path = received_index_path(&repo);
        fault(&path);
        let label = format!("received index, damaged entry / {fault_name}");

        let before = support::store_bytes(&repo);
        let (code, text, _) = run(
            &repo,
            &["bundle", "import", "--input", bundle.to_str().unwrap()],
        );
        if code.is_some_and(|code| code == 0) {
            failures.push(format!(
                "{label}: re-importing must refuse on the damaged entry, but exited 0\n{text}"
            ));
        }
        if !text.contains("has a damaged entry") {
            failures.push(format!(
                "{label}: the refusal must name the damaged entry\n{text}"
            ));
        }
        let after = support::store_bytes(&repo);
        if before != after {
            let mut changed: Vec<String> = before
                .keys()
                .chain(after.keys())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .filter(|path| before.get(*path) != after.get(*path))
                .map(|path| path.display().to_string())
                .collect();
            changed.sort();
            changed.dedup();
            failures.push(format!(
                "{label}: every file under .prikk/ must be byte-identical after the refusal; changed: {changed:?}"
            ));
        }

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(bundle.parent().unwrap());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

// ---------------------------------------------------------------------------------------------
// 6. Prove the rule at every guarded writer, not only the received index (handoff §2): the same
//    "100 zero bytes" fault against each of the six files RFC 163 guards, plus the three
//    generation logs, then that file's own ordinary write. Only the received index used to fail
//    this row (N9); the other seven already held it, via their own pre-existing `has_item_failure`
//    (the four N2-shape files and the received index after this fix) or RFC 162 rule 3 (the
//    pointer index) or the generation-log guard's own damaged-record check (§9).
// ---------------------------------------------------------------------------------------------

fn generation_log_repository_for(target: &str, tag: &str) -> (PathBuf, PathBuf) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    let log_path = match target {
        "pointer index" => {
            std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
            support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
            support::ok(&support::seal(&repo, "heads/main"), "seal");
            let (code, text, _) = run(&repo, &["compact", "--pointer-index"]);
            assert_eq!(code, Some(0), "first compact (pointer index): {text}");
            repo.join(".prikk/refs/containers/pointer-index-generation.log")
        }
        "received index" => {
            let bundle = export_a_bundle(tag);
            support::trust_maintainer(&repo);
            support::ok(
                &support::prikk(&repo)
                    .args(["bundle", "import", "--input", bundle.to_str().unwrap()])
                    .output()
                    .unwrap(),
                "bundle import",
            );
            let (code, text, _) = run(&repo, &["compact", "--received-index"]);
            assert_eq!(code, Some(0), "first compact (received index): {text}");
            repo.join(".prikk/refs/containers/received-index-generation.log")
        }
        "trust policy" => {
            support::trust_maintainer(&repo);
            let (code, text, _) = run(&repo, &["compact", "--trust-policy"]);
            assert_eq!(code, Some(0), "first compact (trust policy): {text}");
            repo.join(".prikk/trust/policy-generation.log")
        }
        _ => unreachable!(),
    };
    (log_path, repo)
}

#[test]
fn every_rfc163_guarded_writer_refuses_on_a_damaged_guarded_file_and_leaves_it_unchanged() {
    let mut failures = Vec::new();
    let mut not_held: Vec<String> = Vec::new();

    // The five ordinary files.
    {
        let repo = pointer_index_repository("rfc163-n9-sweep-pointer-index");
        let path = pointer_index_path(&repo);
        append_zeros_100(&path);
        let corrupted = read_bytes(&path);
        let (code, text) = pointer_index_write(&repo, "branch create", "n9sweep");
        if code.is_some_and(|code| code == 0) || read_bytes(&path) != corrupted {
            not_held.push(format!(
                "pointer index (branch create): exit {code:?}\n{text}"
            ));
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
    {
        let repo = trust_repository("rfc163-n9-sweep-trust-keys");
        let path = trust_key_path(&repo);
        append_zeros_100(&path);
        let corrupted = read_bytes(&path);
        let (code, text) = add_second_maintainer(&repo);
        if code.is_some_and(|code| code == 0) || read_bytes(&path) != corrupted {
            not_held.push(format!(
                "trust keys (trust maintainer add): exit {code:?}\n{text}"
            ));
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
    {
        let repo = trust_repository("rfc163-n9-sweep-trust-policy");
        let path = trust_policy_path(&repo);
        append_zeros_100(&path);
        let corrupted = read_bytes(&path);
        let (code, text) = add_second_maintainer(&repo);
        if code.is_some_and(|code| code == 0) || read_bytes(&path) != corrupted {
            not_held.push(format!(
                "trust policy (trust maintainer add): exit {code:?}\n{text}"
            ));
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
    {
        let repo = author_key_repository("rfc163-n9-sweep-author-keys");
        let path = author_key_path(&repo);
        append_zeros_100(&path);
        let corrupted = read_bytes(&path);
        std::fs::write(repo.join("by-second.txt"), "second\n".repeat(5)).unwrap();
        let (code, text) = commit_by_second_author(&repo, "n9 sweep, second author");
        if code.is_some_and(|code| code == 0) || read_bytes(&path) != corrupted {
            not_held.push(format!(
                "author keys (a new author's commit): exit {code:?}\n{text}"
            ));
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
    {
        let (repo, bundle) = received_index_repository("rfc163-n9-sweep-received-index");
        let path = received_index_path(&repo);
        append_zeros_100(&path);
        let corrupted = read_bytes(&path);
        let (code, text, _) = run(
            &repo,
            &["bundle", "import", "--input", bundle.to_str().unwrap()],
        );
        if code.is_some_and(|code| code == 0) || read_bytes(&path) != corrupted {
            not_held.push(format!(
                "received index (bundle import): exit {code:?}\n{text}"
            ));
        }
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(bundle.parent().unwrap());
    }

    // The three generation logs.
    for target in ["pointer index", "received index", "trust policy"] {
        let (log_path, repo) = generation_log_repository_for(
            target,
            &format!("rfc163-n9-sweep-genlog-{}", target.replace(' ', "-")),
        );
        append_zeros_100(&log_path);
        let corrupted = read_bytes(&log_path);
        let flag = match target {
            "pointer index" => "--pointer-index",
            "received index" => "--received-index",
            "trust policy" => "--trust-policy",
            _ => unreachable!(),
        };
        let (code, text, _) = run(&repo, &["compact", flag]);
        if code.is_some_and(|code| code == 0) || read_bytes(&log_path) != corrupted {
            not_held.push(format!(
                "{target}'s generation log (compact {flag}): exit {code:?}\n{text}"
            ));
        }
        let _ = std::fs::remove_dir_all(&repo);
    }

    if !not_held.is_empty() {
        failures.push(format!(
            "the following did NOT hold the rule (refuse and leave the guarded file unchanged) \
             against 100 zero bytes:\n{}",
            not_held.join("\n---\n")
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}
