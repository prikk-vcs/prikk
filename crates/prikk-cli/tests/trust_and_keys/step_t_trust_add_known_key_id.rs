//! 0.56.0 step T: `trust maintainer add` under a key id the repository already knows, through the
//! compiled binary.
//!
//! A key id stays bound to the public key it was first adopted with, **even after
//! `trust maintainer remove`** -- deliberately, because the id travels in every signature. The
//! refusal used to say "already adopted" for a removed id too, which `trust maintainer list`
//! contradicts. These controls pin the three states (adopted, removed, removed then re-added with
//! its original key), the `refusal-v1` id of each refusal (the **JSON id**, not only the sentence),
//! that a refusal writes nothing, and `--format json`'s success document.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::json::{Value, parse};
use std::path::Path;
use std::process::Output;

fn key_hex(seed: &[u8; 32]) -> String {
    use prikk_store::MaintainerSigner;
    let signer = prikk_store::Ed25519MaintainerSigner::from_seed("probe", seed)
        .expect("fixed test seed derives a valid signer");
    support::hex(&signer.public_key_bytes())
}

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn add(repo: &Path, key_id: &str, public_key: &str, format_json: bool) -> Output {
    let mut args = vec![
        "trust",
        "maintainer",
        "add",
        "--key-id",
        key_id,
        "--public-key",
        public_key,
    ];
    if format_json {
        args.extend(["--format", "json"]);
    }
    run(repo, &args)
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn list_json(repo: &Path) -> String {
    let out = run(repo, &["trust", "maintainer", "list", "--format", "json"]);
    support::ok(&out, "trust maintainer list");
    stdout_of(&out)
}

/// `x` adopted with key A and `z` with key B, so `x` can be removed (the last key cannot be).
fn repo_with_x_and_z(tag: &str) -> (std::path::PathBuf, String, String) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    let key_a = key_hex(&[0x0a; 32]);
    let key_b = key_hex(&[0x0b; 32]);
    support::ok(&add(&repo, "x", &key_a, false), "adopt x");
    support::ok(&add(&repo, "z", &key_b, false), "adopt z");
    (repo, key_a, key_b)
}

fn refusal(out: &Output) -> Value {
    let stdout = stdout_of(out);
    let doc = parse(&stdout);
    assert_eq!(doc.get("schema_version").as_str(), "refusal-v1");
    assert_eq!(doc.get("command").as_str(), "trust-maintainer-add");
    assert!(doc.get("reason").is_null());
    doc
}

/// An id that is in the adopted set, offered a different key: refused as before, and under
/// `--format json` the document carries the adopted-id refusal.
#[test]
fn an_adopted_id_with_a_different_key_is_refused_with_the_adopted_id() {
    let (repo, _a, _b) = repo_with_x_and_z("step-t-adopted");
    let other = key_hex(&[0x0c; 32]);
    let before = support::store_bytes(&repo);
    let list_before = list_json(&repo);

    let out = add(&repo, "x", &other, false);
    let stderr = stderr_of(&out);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr.contains("error: precondition not met:")
            && stderr
                .contains("maintainer key id x is already adopted with a different public key"),
        "{stderr}"
    );
    assert!(stdout_of(&out).is_empty(), "no document without the flag");

    let out = add(&repo, "x", &other, true);
    assert_eq!(out.status.code(), Some(1));
    let doc = refusal(&out);
    assert_eq!(
        doc.get("refusal").as_str(),
        "maintainer-key-id-adopted-with-different-key"
    );
    assert!(
        doc.get("message")
            .as_str()
            .starts_with("precondition not met: maintainer key id x is already adopted"),
        "{}",
        stdout_of(&out)
    );
    assert!(
        stderr_of(&out).starts_with("error: precondition not met:"),
        "stderr keeps its own text under --format json"
    );

    assert_eq!(
        support::store_bytes(&repo),
        before,
        "a refusal writes nothing"
    );
    assert_eq!(list_json(&repo), list_before);
    let _ = std::fs::remove_dir_all(&repo);
}

/// An id that was removed, offered a different key: the message says it is not adopted but stays
/// bound, names the distinct-id route, and does not claim "already adopted". The document carries
/// the removed-id refusal, which is not the adopted-id one.
#[test]
fn a_removed_id_with_a_different_key_is_refused_truthfully_with_its_own_id() {
    let (repo, _a, b) = repo_with_x_and_z("step-t-removed");
    support::ok(
        &run(&repo, &["trust", "maintainer", "remove", "--key-id", "x"]),
        "remove x",
    );
    let list_before = list_json(&repo);
    assert!(
        !list_before.contains("\"x\""),
        "x is not adopted: {list_before}"
    );
    let before = support::store_bytes(&repo);

    let out = add(&repo, "x", &b, false);
    let stderr = stderr_of(&out);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr.starts_with("error: precondition not met:"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("already adopted"),
        "x is not adopted, and the line must not say so: {stderr}"
    );
    assert!(
        stderr.contains("maintainer key id x is not currently adopted")
            && stderr.contains("stays bound to the public key it was first adopted with")
            && stderr.contains("A key id travels in every signature")
            && stderr.contains("original public key")
            && stderr.contains("prikk key generate --out <path>")
            && stderr.contains("PRIKK_MAINTAINER_KEY_ID")
            && stderr.contains("prikk trust maintainer add --key-id <that id> --public-key"),
        "{stderr}"
    );

    let out = add(&repo, "x", &b, true);
    assert_eq!(out.status.code(), Some(1));
    let doc = refusal(&out);
    assert_eq!(
        doc.get("refusal").as_str(),
        "maintainer-key-id-bound-to-removed-key"
    );
    assert!(
        !doc.get("message").as_str().contains("already adopted"),
        "{}",
        stdout_of(&out)
    );

    assert_eq!(
        support::store_bytes(&repo),
        before,
        "a refusal writes nothing"
    );
    assert_eq!(
        list_json(&repo),
        list_before,
        "the adopted set is unchanged"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// A removed id re-added with its **original** key still works, in both forms. `newly_added` is what
/// the code really reports: `true`, because the adopted set gained the id (the key material was
/// already on record and is not rewritten).
#[test]
fn a_removed_id_readded_with_its_original_key_still_works() {
    let (repo, a, _b) = repo_with_x_and_z("step-t-readd");
    support::ok(
        &run(&repo, &["trust", "maintainer", "remove", "--key-id", "x"]),
        "remove x",
    );

    let out = add(&repo, "x", &a, true);
    support::ok(&out, "re-add x with its original key");
    let doc = parse(&stdout_of(&out));
    assert_eq!(doc.get("schema_version").as_str(), "trust-add-v1");
    assert_eq!(doc.get("key_id").as_str(), "x");
    assert_eq!(doc.get("public_key").as_str(), a);
    assert!(doc.get("newly_added").as_bool());
    assert_eq!(doc.get("adopted_count"), &Value::Number("2".to_string()));
    assert!(list_json(&repo).contains("\"x\""));

    // And the prose form, after a second removal.
    support::ok(
        &run(&repo, &["trust", "maintainer", "remove", "--key-id", "x"]),
        "remove x again",
    );
    let out = add(&repo, "x", &a, false);
    support::ok(&out, "re-add x, prose");
    assert_eq!(
        stdout_of(&out),
        "trusted maintainer key: x\nadopted maintainer keys: 2\n"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// `--format json` on success: a fresh add and the idempotent re-add, one document and nothing else.
#[test]
fn format_json_success_documents_a_fresh_add_and_an_idempotent_readd() {
    let repo = support::unique_repo("step-t-json-success");
    support::init(&repo);
    let a = key_hex(&[0x0a; 32]);

    let out = add(&repo, "x", &a, true);
    support::ok(&out, "fresh add");
    assert_eq!(
        stdout_of(&out),
        format!(
            "{{\n  \"schema_version\": \"trust-add-v1\",\n  \"key_id\": \"x\",\n  \
             \"public_key\": \"{a}\",\n  \"newly_added\": true,\n  \"adopted_count\": 1\n}}\n"
        )
    );

    let out = add(&repo, "x", &a, true);
    support::ok(&out, "idempotent re-add");
    let doc = parse(&stdout_of(&out));
    assert_eq!(doc.get("schema_version").as_str(), "trust-add-v1");
    assert!(!doc.get("newly_added").as_bool());
    assert_eq!(doc.get("adopted_count"), &Value::Number("1".to_string()));
    let _ = std::fs::remove_dir_all(&repo);
}

/// The flag is parsed like its siblings': only `json`, and only once. Other failures print no
/// `refusal-v1` document under the flag (a malformed key is not a bound-id refusal).
#[test]
fn format_flag_is_parsed_like_the_siblings_and_other_failures_print_no_document() {
    let repo = support::unique_repo("step-t-json-parse");
    support::init(&repo);
    let a = key_hex(&[0x0a; 32]);

    let out = run(
        &repo,
        &[
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "x",
            "--public-key",
            &a,
            "--format",
            "xml",
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr_of(&out).contains("trust maintainer add --format does not support \"xml\""),
        "{}",
        stderr_of(&out)
    );
    let out = run(
        &repo,
        &[
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "x",
            "--public-key",
            &a,
            "--format",
            "json",
            "--format",
            "json",
        ],
    );
    assert_eq!(out.status.code(), Some(2));

    let out = add(&repo, "x", "not-hex", true);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout_of(&out).is_empty(), "{}", stdout_of(&out));
    assert!(!support::store_bytes(&repo).is_empty());
    let _ = std::fs::remove_dir_all(&repo);
}
